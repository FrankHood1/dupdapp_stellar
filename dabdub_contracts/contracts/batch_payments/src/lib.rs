#![no_std]

mod test;

use soroban_sdk::{
    contract, contractimpl, contracterror, contracttype, xdr::ToXdr, Address, Bytes, BytesN, Env,
    String, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InvalidLimits = 3,
    ContractNotInitialized = 4,
    BatchEmpty = 5,
    BatchTooLarge = 6,
    PaymentAmountOutsideLimits = 7,
    MemoEmpty = 8,
    NotAdmin = 9,
}

const MAX_BATCH_SIZE: u32 = 20;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    MinAmount,
    MaxAmount,
    Counter,
    Payment(BytesN<32>),
}

/// A single payment input in the batch.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PaymentInput {
    /// Amount in stroops (must be > 0).
    pub amount: i128,
    /// Non-empty human-readable memo for the payment.
    pub memo: String,
    /// Optional customer Stellar address.
    pub customer: Option<Address>,
}

/// The result record for each created payment.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PaymentRecord {
    /// Unique payment ID (32-byte hash derived from merchant, a per-contract
    /// counter, and the payment's contents — see issue #1024).
    pub id: BytesN<32>,
    pub amount: i128,
    pub memo: String,
    pub merchant: Address,
}

#[contract]
pub struct BatchPaymentContract;

#[contractimpl]
impl BatchPaymentContract {
    /// One-time initialization for admin and amount limits.
    pub fn init(env: Env, admin: Address, min_amount: i128, max_amount: i128) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        if min_amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if min_amount > max_amount {
            return Err(ContractError::InvalidLimits);
        }

        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::MinAmount, &min_amount);
        env.storage().instance().set(&DataKey::MaxAmount, &max_amount);
        env.storage().instance().set(&DataKey::Counter, &0u64);
        Ok(())
    }

    /// Update payment amount limits. Admin-only.
    pub fn set_limits(env: Env, min_amount: i128, max_amount: i128) -> Result<(), ContractError> {
        if min_amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if min_amount > max_amount {
            return Err(ContractError::InvalidLimits);
        }

        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::ContractNotInitialized)?;
        admin.require_auth();

        env.storage().instance().set(&DataKey::MinAmount, &min_amount);
        env.storage().instance().set(&DataKey::MaxAmount, &max_amount);
        env.events().publish(
            (soroban_sdk::Symbol::new(&env, "LimitsUpdated"),),
            (min_amount, max_amount),
        );
        Ok(())
    }

    /// Create up to 20 payments atomically in a single contract invocation.
    ///
    /// Validates every input before any state is written — if any item is
    /// invalid the entire batch reverts. Emits a `PaymentCreated` event for
    /// each created payment, matching the NestJS service event log.
    ///
    /// Returns a `Vec<BytesN<32>>` of the created payment IDs.
    pub fn create_batch(
        env: Env,
        merchant: Address,
        payments: Vec<PaymentInput>,
    ) -> Result<Vec<BytesN<32>>, ContractError> {
        merchant.require_auth();

        let min_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::MinAmount)
            .ok_or(ContractError::ContractNotInitialized)?;
        let max_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::MaxAmount)
            .ok_or(ContractError::ContractNotInitialized)?;

        let count = payments.len();
        if count == 0 {
            return Err(ContractError::BatchEmpty);
        }
        if count > MAX_BATCH_SIZE {
            return Err(ContractError::BatchTooLarge);
        }

        // ── Validation pass (all items checked before any state write) ────────
        for i in 0..count {
            let item = payments.get(i).unwrap();
            if item.amount < min_amount || item.amount > max_amount {
                return Err(ContractError::PaymentAmountOutsideLimits);
            }
            if item.memo.len() == 0 {
                return Err(ContractError::MemoEmpty);
            }
        }

        // ── Creation pass ─────────────────────────────────────────────────────
        // ID derivation scheme (issue #1201):
        //   sha256(merchant_xdr || ledger_sequence || ledger_timestamp || counter)
        // - merchant_xdr: prevents collisions across different merchants
        // - ledger_sequence: bounds IDs to a ledger window
        // - ledger_timestamp: extra entropy when sequence resets in test networks
        // - counter: monotonically increasing, ensures no two items in the same
        //   batch share an ID even when all other inputs are identical
        let mut payment_ids: Vec<BytesN<32>> = Vec::new(&env);
        let mut counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::Counter)
            .unwrap_or(0u64);
        let ledger_seq = env.ledger().sequence() as u64;
        let ledger_ts = env.ledger().timestamp();
        let merchant_xdr = merchant.clone().to_xdr(&env);

        for i in 0..count {
            let item = payments.get(i).unwrap();

            let new_counter = counter + i as u64;
            let mut seed_bytes = merchant_xdr.clone();
            seed_bytes.append(&Bytes::from_array(&env, &ledger_seq.to_be_bytes()));
            seed_bytes.append(&Bytes::from_array(&env, &ledger_ts.to_be_bytes()));
            seed_bytes.append(&Bytes::from_array(&env, &new_counter.to_be_bytes()));

            let id_bytes: BytesN<32> = env
                .crypto()
                .sha256(&seed_bytes)
                .into();

            // Emit PaymentCreated event — one per batch entry.
            env.events().publish(
                (soroban_sdk::Symbol::new(&env, "PaymentCreated"),),
                (id_bytes.clone(), merchant.clone(), item.amount, item.memo.clone()),
            );

            payment_ids.push_back(id_bytes);
        }

        let final_counter = counter + count as u64;
        env.storage().instance().set(&DataKey::Counter, &final_counter);

        Ok(payment_ids)
    }

    /// Returns the on-chain record for a previously created payment, if any.
    pub fn get_payment(env: Env, id: BytesN<32>) -> Option<PaymentRecord> {
        env.storage().persistent().get(&DataKey::Payment(id))
    }

    /// Returns the maximum allowed batch size.
    pub fn max_batch_size(_env: Env) -> u32 {
        MAX_BATCH_SIZE
    }

    /// Return the currently configured min and max payment amounts.
    pub fn get_limits(env: Env) -> Result<(i128, i128), ContractError> {
        let min_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::MinAmount)
            .ok_or(ContractError::ContractNotInitialized)?;
        let max_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::MaxAmount)
            .ok_or(ContractError::ContractNotInitialized)?;
        Ok((min_amount, max_amount))
    }
}
