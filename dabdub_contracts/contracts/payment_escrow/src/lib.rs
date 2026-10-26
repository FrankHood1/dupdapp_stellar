#![no_std]

mod test;

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, token, Address, BytesN,
    Env, String, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    InvalidTtl = 1,
    TtlExceedsMaximum = 2,
    InvalidAmount = 3,
    DuplicatePaymentId = 4,
    MerchantNotApproved = 5,
    PaymentNotFound = 6,
    PaymentFullyReleased = 7,
    PaymentExpired = 8,
    PaymentHasNotExpired = 9,
    PaymentNotPending = 10,
    DisputeOpen = 11,
    DisputeNotOpen = 12,
    NotPaymentParticipant = 13,
    DisputeWindowExpired = 14,
    InvalidDisputeWinner = 15,
    NotAdmin = 16,
    ReleaseFundsExceedRemaining = 17,
    NotEmergencySigner = 18,
    EmergencyDrainCooldownActive = 19,
    NoFundsToDrain = 20,
    InvalidEmergencyCooldown = 21,
    InvalidSignerCount = 22,
    DuplicateEmergencySigners = 23,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum AssetType {
    Xlm,
    Usdc,
}

/// Thin client interface for the MerchantRegistry contract.
#[contractclient(name = "MerchantRegistryClient")]
#[allow(dead_code)]
trait MerchantRegistry {
    fn is_approved(env: Env, merchant: Address) -> bool;
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum PaymentStatus {
    Pending,
    Disputed,
    Released,
    Expired,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PaymentEscrow {
    pub payment_id: BytesN<32>,
    pub amount: i128,
    pub released_amount: i128,
    pub merchant: Address,
    pub customer: Address,
    pub status: PaymentStatus,
    pub expiry: u32,
    pub dispute_window_end: u32,
    pub dispute_reason: Option<String>,
    pub asset_type: AssetType,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    XlmToken,
    UsdcToken,
    DefaultTtlLedgers,
    RegistryContract,
    EmergencySigners,
    EmergencyTreasury,
    EmergencyCooldownLedgers,
    EmergencyLastDrainLedger,
    Payment(BytesN<32>),
    Version,
}

#[contracttype]
struct DepositEvent {
    payment_id: BytesN<32>,
    customer: Address,
    merchant: Address,
    amount: i128,
    expiry: u32,
}

#[contracttype]
struct EscrowStateEvent {
    payment_id: BytesN<32>,
    amount: i128,
}

#[contracttype]
struct EmergencyDrainEvent {
    amount: i128,
    caller: Address,
}

const MAX_DISPUTE_WINDOW_LEDGERS: u32 = 51_840;
const MAX_TTL_LEDGERS: u32 = 518_400;
const EMERGENCY_SIGNER_COUNT: u32 = 3;

#[contract]
pub struct PaymentEscrowContract;

#[contractimpl]
impl PaymentEscrowContract {
    pub fn __constructor(
        env: Env,
        admin: Address,
        xlm_token: Address,
        usdc_token: Address,
        default_ttl_ledgers: u32,
        registry: Option<Address>,
        emergency_signers: Vec<Address>,
        emergency_treasury: Address,
        emergency_cooldown_ledgers: u32,
    ) -> Result<(), ContractError> {
        if default_ttl_ledgers == 0 {
            return Err(ContractError::InvalidTtl);
        }
        if emergency_cooldown_ledgers == 0 {
            return Err(ContractError::InvalidEmergencyCooldown);
        }
        if emergency_signers.len() != EMERGENCY_SIGNER_COUNT {
            return Err(ContractError::InvalidSignerCount);
        }
        let signer_0 = emergency_signers.get(0).unwrap();
        let signer_1 = emergency_signers.get(1).unwrap();
        let signer_2 = emergency_signers.get(2).unwrap();
        if signer_0 == signer_1 || signer_0 == signer_2 || signer_1 == signer_2 {
            return Err(ContractError::DuplicateEmergencySigners);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::XlmToken, &xlm_token);
        env.storage().instance().set(&DataKey::UsdcToken, &usdc_token);
        env.storage()
            .instance()
            .set(&DataKey::DefaultTtlLedgers, &default_ttl_ledgers);
        if let Some(reg) = registry {
            env.storage()
                .instance()
                .set(&DataKey::RegistryContract, &reg);
        }
        env.storage()
            .instance()
            .set(&DataKey::EmergencySigners, &emergency_signers);
        env.storage()
            .instance()
            .set(&DataKey::EmergencyTreasury, &emergency_treasury);
        env.storage()
            .instance()
            .set(&DataKey::EmergencyCooldownLedgers, &emergency_cooldown_ledgers);
        env.storage()
            .instance()
            .set(&DataKey::EmergencyLastDrainLedger, &0u32);

        // Initialize contract version to 1.
        env.storage().instance().set(&DataKey::Version, &1u32);
        Ok(())
    }

    /// Upgrade the contract WASM. Only callable by the admin.
    pub fn upgrade(env: Env, caller: Address, new_wasm_hash: BytesN<32>) -> Result<(), ContractError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let version: u32 = env.storage().instance().get(&DataKey::Version).unwrap_or(0);
        env.storage().instance().set(&DataKey::Version, &(version + 1));

        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    /// Return the current contract version.
    pub fn get_version(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::Version).unwrap_or(0)
    }

    /// Update (or remove) the merchant registry address.  Admin-only.
    pub fn set_registry(env: Env, caller: Address, registry: Option<Address>) -> Result<(), ContractError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        match registry {
            Some(reg) => env
                .storage()
                .instance()
                .set(&DataKey::RegistryContract, &reg),
            None => env.storage().instance().remove(&DataKey::RegistryContract),
        }
        Ok(())
    }

    /// Return the registry contract address if one is configured.
    pub fn get_registry(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::RegistryContract)
    }

    pub fn deposit(
        env: Env,
        customer: Address,
        payment_id: BytesN<32>,
        merchant: Address,
        amount: i128,
        ttl_ledgers: u32,
        asset_type: AssetType,
    ) -> Result<BytesN<32>, ContractError> {
        customer.require_auth();

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if ttl_ledgers == 0 {
            return Err(ContractError::InvalidTtl);
        }
        if ttl_ledgers > MAX_TTL_LEDGERS {
            return Err(ContractError::TtlExceedsMaximum);
        }

        let key = DataKey::Payment(payment_id.clone());
        if env.storage().persistent().has(&key) {
            return Err(ContractError::DuplicatePaymentId);
        }

        // If a registry contract is configured, verify the merchant is approved.
        if let Some(registry_addr) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::RegistryContract)
        {
            let registry_client = MerchantRegistryClient::new(&env, &registry_addr);
            if !registry_client.is_approved(&merchant) {
                return Err(ContractError::MerchantNotApproved);
            }
        }

        // Transfer funds into the contract via the appropriate token interface.
        let token_addr = Self::token_address(&env, &asset_type);
        token::Client::new(&env, &token_addr).transfer(
            &customer,
            &env.current_contract_address(),
            &amount,
        );

        let expiry = env.ledger().sequence().saturating_add(ttl_ledgers);
        let dispute_window_end = {
            let max_window_end = env
                .ledger()
                .sequence()
                .saturating_add(MAX_DISPUTE_WINDOW_LEDGERS);
            if expiry < max_window_end {
                expiry
            } else {
                max_window_end
            }
        };

        let payment = PaymentEscrow {
            payment_id: payment_id.clone(),
            amount,
            released_amount: 0,
            merchant: merchant.clone(),
            customer: customer.clone(),
            status: PaymentStatus::Pending,
            expiry,
            dispute_window_end,
            dispute_reason: None,
            asset_type,
        };

        env.storage().persistent().set(&key, &payment);

        env.events().publish(
            ("ESCROW", "deposit"),
            DepositEvent {
                payment_id: payment_id.clone(),
                customer,
                merchant,
                amount,
                expiry,
            },
        );

        Ok(payment_id)
    }

    pub fn release(env: Env, caller: Address, payment_id: BytesN<32>) -> Result<(), ContractError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut payment = Self::get_payment(env.clone(), payment_id.clone())?;
        Self::require_releasable(&env, &payment)?;

        let remaining = Self::remaining_amount(&payment);
        if remaining <= 0 {
            return Err(ContractError::PaymentFullyReleased);
        }

        Self::transfer_from_contract(&env, &payment.merchant, remaining, &payment.asset_type);
        payment.released_amount = payment.amount;
        payment.status = PaymentStatus::Released;
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id.clone()), &payment);

        env.events().publish(
            ("ESCROW", "release"),
            EscrowStateEvent {
                payment_id,
                amount: remaining,
            },
        );
        Ok(())
    }

    pub fn release_partial(env: Env, caller: Address, payment_id: BytesN<32>, amount: i128) -> Result<(), ContractError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let mut payment = Self::get_payment(env.clone(), payment_id.clone())?;
        Self::require_releasable(&env, &payment)?;

        let remaining = Self::remaining_amount(&payment);
        if remaining <= 0 {
            return Err(ContractError::PaymentFullyReleased);
        }
        if amount > remaining {
            return Err(ContractError::ReleaseFundsExceedRemaining);
        }

        Self::transfer_from_contract(&env, &payment.merchant, amount, &payment.asset_type);
        payment.released_amount += amount;
        if payment.released_amount == payment.amount {
            payment.status = PaymentStatus::Released;
        }
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id.clone()), &payment);

        env.events().publish(
            ("ESCROW", "release"),
            EscrowStateEvent {
                payment_id,
                amount,
            },
        );
        Ok(())
    }

    pub fn expire(env: Env, payment_id: BytesN<32>) -> Result<(), ContractError> {
        let mut payment = Self::get_payment(env.clone(), payment_id.clone())?;
        Self::require_expirable(&env, &payment)?;

        let remaining = Self::remaining_amount(&payment);
        if remaining <= 0 {
            return Err(ContractError::PaymentFullyReleased);
        }
        Self::transfer_from_contract(&env, &payment.customer, remaining, &payment.asset_type);

        payment.released_amount = payment.amount;
        payment.status = PaymentStatus::Expired;
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id.clone()), &payment);

        env.events().publish(
            ("ESCROW", "expired"),
            EscrowStateEvent {
                payment_id,
                amount: remaining,
            },
        );
        Ok(())
    }

    pub fn refund(env: Env, payment_id: BytesN<32>) -> Result<(), ContractError> {
        let mut payment = Self::get_payment(env.clone(), payment_id.clone())?;
        Self::require_expirable(&env, &payment)?;

        let remaining = Self::remaining_amount(&payment);
        if remaining <= 0 {
            return Err(ContractError::PaymentFullyReleased);
        }

        Self::transfer_from_contract(&env, &payment.customer, remaining, &payment.asset_type);
        payment.released_amount = payment.amount;
        payment.status = PaymentStatus::Expired;
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id.clone()), &payment);

        env.events().publish(
            ("ESCROW", "refund"),
            EscrowStateEvent {
                payment_id,
                amount: remaining,
            },
        );
        Ok(())
    }

    /// Opens a dispute on a payment still in `Pending` status.
    ///
    /// Note (#1023): `release_partial` keeps a payment `Pending` until it is
    /// fully released, so a payment that already had one or more partial
    /// releases sent to the merchant can still be disputed here. This is
    /// intentional: `resolve_dispute` only ever moves `remaining_amount()`
    /// (`amount - released_amount`) to the winner, so a customer disputing
    /// after accepting partial delivery recovers only the undelivered
    /// remainder — funds already released to the merchant are not clawed
    /// back. See `test_dispute_after_partial_release_resolves_to_customer`
    /// and `test_dispute_after_partial_release_resolves_to_merchant`.
    pub fn dispute(env: Env, caller: Address, payment_id: BytesN<32>, _reason: String) -> Result<(), ContractError> {
        caller.require_auth();

        let mut payment = Self::get_payment(env.clone(), payment_id.clone())?;
        if payment.status == PaymentStatus::Disputed {
            return Err(ContractError::DisputeOpen);
        }
        if payment.status != PaymentStatus::Pending {
            return Err(ContractError::PaymentNotPending);
        }
        if caller != payment.customer && caller != payment.merchant {
            return Err(ContractError::NotPaymentParticipant);
        }
        if env.ledger().sequence() > payment.dispute_window_end {
            return Err(ContractError::DisputeWindowExpired);
        }

        payment.status = PaymentStatus::Disputed;
        payment.dispute_reason = Some(_reason.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id.clone()), &payment);

        env.events().publish(
            ("ESCROW", "dispute"),
            EscrowStateEvent {
                payment_id,
                amount: Self::remaining_amount(&payment),
            },
        );
        Ok(())
    }

    pub fn resolve_dispute(env: Env, caller: Address, payment_id: BytesN<32>, winner: Address) -> Result<(), ContractError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let mut payment = Self::get_payment(env.clone(), payment_id.clone())?;
        if payment.status != PaymentStatus::Disputed {
            return Err(ContractError::DisputeNotOpen);
        }
        if winner != payment.customer && winner != payment.merchant {
            return Err(ContractError::InvalidDisputeWinner);
        }

        let remaining = Self::remaining_amount(&payment);
        if remaining <= 0 {
            return Err(ContractError::PaymentFullyReleased);
        }

        Self::transfer_from_contract(&env, &winner, remaining, &payment.asset_type);
        payment.status = if winner == payment.merchant {
            PaymentStatus::Released
        } else {
            PaymentStatus::Expired
        };
        payment.released_amount = payment.amount;
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id.clone()), &payment);

        let topic = if winner == payment.merchant {
            "release"
        } else {
            "refund"
        };
        env.events().publish(
            ("ESCROW", topic),
            EscrowStateEvent {
                payment_id,
                amount: remaining,
            },
        );
        Ok(())
    }

    pub fn get_payment(env: Env, payment_id: BytesN<32>) -> Result<PaymentEscrow, ContractError> {
        env.storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .ok_or(ContractError::PaymentNotFound)
    }

    pub fn get_balance(env: Env, payment_id: BytesN<32>) -> Result<i128, ContractError> {
        let payment = Self::get_payment(env, payment_id)?;
        Ok(Self::remaining_amount(&payment))
    }

    pub fn get_expiry(env: Env, payment_id: BytesN<32>) -> Result<u32, ContractError> {
        Ok(Self::get_payment(env, payment_id)?.expiry)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_xlm_token(env: Env) -> Address {
        env.storage().instance().get(&DataKey::XlmToken).unwrap()
    }

    pub fn get_usdc_token(env: Env) -> Address {
        env.storage().instance().get(&DataKey::UsdcToken).unwrap()
    }

    pub fn get_default_ttl_ledgers(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::DefaultTtlLedgers)
            .unwrap()
    }

    pub fn get_max_dispute_window_ledgers(_env: Env) -> u32 {
        MAX_DISPUTE_WINDOW_LEDGERS
    }

    pub fn get_max_ttl_ledgers(_env: Env) -> u32 {
        MAX_TTL_LEDGERS
    }

    pub fn emergency_drain(env: Env, caller: Address, signer_one: Address, signer_two: Address) -> Result<i128, ContractError> {
        caller.require_auth();
        if signer_one == signer_two {
            return Err(ContractError::DuplicateEmergencySigners);
        }
        signer_one.require_auth();
        signer_two.require_auth();
        Self::require_emergency_signer(&env, &signer_one)?;
        Self::require_emergency_signer(&env, &signer_two)?;

        let current_ledger = env.ledger().sequence();
        let last_drain_ledger: u32 = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyLastDrainLedger)
            .unwrap_or(0);
        let cooldown_ledgers: u32 = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyCooldownLedgers)
            .unwrap();
        if last_drain_ledger != 0
            && current_ledger < last_drain_ledger.saturating_add(cooldown_ledgers)
        {
            return Err(ContractError::EmergencyDrainCooldownActive);
        }

        let usdc_token: Address = env.storage().instance().get(&DataKey::UsdcToken).unwrap();
        let treasury: Address = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyTreasury)
            .unwrap();
        let token_client = token::Client::new(&env, &usdc_token);
        let contract_address = env.current_contract_address();
        let amount = token_client.balance(&contract_address);
        if amount <= 0 {
            return Err(ContractError::NoFundsToDrain);
        }

        token_client.transfer(&contract_address, &treasury, &amount);
        env.storage()
            .instance()
            .set(&DataKey::EmergencyLastDrainLedger, &current_ledger);
        env.events().publish(
            ("ESCROW", "emergency_drain"),
            EmergencyDrainEvent { amount, caller },
        );

        Ok(amount)
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), ContractError> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != &admin {
            return Err(ContractError::NotAdmin);
        }
        Ok(())
    }

    fn require_releasable(env: &Env, payment: &PaymentEscrow) -> Result<(), ContractError> {
        if payment.status == PaymentStatus::Disputed {
            return Err(ContractError::DisputeOpen);
        }
        if payment.status == PaymentStatus::Released {
            return Err(ContractError::PaymentFullyReleased);
        }
        if payment.status == PaymentStatus::Expired {
            return Err(ContractError::PaymentExpired);
        }
        if env.ledger().sequence() > payment.expiry {
            return Err(ContractError::PaymentExpired);
        }
        Ok(())
    }

    fn require_expirable(env: &Env, payment: &PaymentEscrow) -> Result<(), ContractError> {
        if payment.status == PaymentStatus::Disputed {
            return Err(ContractError::DisputeOpen);
        }
        if payment.status == PaymentStatus::Released {
            return Err(ContractError::PaymentFullyReleased);
        }
        if payment.status != PaymentStatus::Pending {
            return Err(ContractError::PaymentNotPending);
        }
        if env.ledger().sequence() <= payment.expiry {
            return Err(ContractError::PaymentHasNotExpired);
        }
        Ok(())
    }

    fn remaining_amount(payment: &PaymentEscrow) -> i128 {
        payment.amount.saturating_sub(payment.released_amount)
    }

    fn token_address(env: &Env, asset_type: &AssetType) -> Address {
        match asset_type {
            AssetType::Xlm => env.storage().instance().get(&DataKey::XlmToken).unwrap(),
            AssetType::Usdc => env.storage().instance().get(&DataKey::UsdcToken).unwrap(),
        }
    }

    fn transfer_from_contract(env: &Env, recipient: &Address, amount: i128, asset_type: &AssetType) {
        let token_addr = Self::token_address(env, asset_type);
        token::Client::new(env, &token_addr)
            .transfer(&env.current_contract_address(), recipient, &amount);
    }

    fn require_emergency_signer(env: &Env, signer: &Address) -> Result<(), ContractError> {
        let signers: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::EmergencySigners)
            .unwrap();
        if signers.iter().all(|configured| configured != *signer) {
            return Err(ContractError::NotEmergencySigner);
        }
        Ok(())
    }
}
