# fee_distributor

Soroban smart contract that atomically splits a collected fee between two recipients — a **treasury** (platform operational wallet) and a **liquidity provider (LP)** — in a single token transfer.

## Purpose

When a payment is settled via `fee_calculator`, the computed fee must be distributed to the platform's revenue recipients. `fee_distributor` enforces this split atomically: both outbound transfers happen in the same Soroban transaction, so partial distributions cannot occur.

## Recipients

| Recipient | Role | Share |
|---|---|---|
| Treasury | Platform operational wallet — funds infrastructure, development, and operations | `(1 - lp_share_bps / 10000)` of total fee |
| LP Address | Liquidity provider — compensates liquidity provisioned via the AMM | `lp_share_bps / 10000` of total fee |

The split ratio is stored as **basis points** (`lp_share_bps`). 5 000 bps = 50% LP / 50% treasury.

### Rounding policy

`lp_amount` truncates via integer division (`total_fee * lp_share_bps / 10_000`). Any remainder is allocated to the treasury. This is an intentional policy choice favoring the treasury — do not change the arithmetic order without an explicit decision.

## Interface

```rust
// Deploy
fn __constructor(
    admin: Address,
    treasury: Address,
    lp_address: Address,
    lp_share_bps: i128,   // 0 – 10_000 inclusive
    usdc_token: Address,
    allowed_caller: Address,
) -> ();

// Distribute total_fee (in USDC stroops) between treasury and LP.
// Only `allowed_caller` may invoke this.
// The caller must hold sufficient USDC balance.
fn distribute(caller: Address, total_fee: i128) -> ();

// Admin-only setters
fn set_lp_share(caller: Address, bps: i128) -> ();
fn set_treasury(caller: Address, treasury: Address) -> ();
fn set_lp_address(caller: Address, lp_address: Address) -> ();
fn set_allowed_caller(caller: Address, allowed_caller: Address) -> ();
fn transfer_admin(caller: Address, new_admin: Address) -> ();

// Read-only
fn get_config() -> (Address, Address, i128);  // (treasury, lp_address, lp_share_bps)
fn get_allowed_caller() -> Address;
```

## Callers

Only the `allowed_caller` address (typically `payment_escrow` or a settlement orchestrator) may call `distribute`. The admin rotates this via `set_allowed_caller`.

## Events

| Topic | Data |
|---|---|
| `("FEE_DISTRIBUTOR", "fee_distributed")` | `{ treasury_amount, lp_amount, lp_share_bps }` |
| `("FEE_DISTRIBUTOR", "lp_share_updated")` | `{ old_bps, new_bps }` |
| `("FEE_DISTRIBUTOR", "treasury_updated")` | `{ old_treasury, new_treasury }` |
| `("FEE_DISTRIBUTOR", "lp_address_updated")` | `{ old_lp_address, new_lp_address }` |
| `("FEE_DISTRIBUTOR", "admin_updated")` | `{ old_admin, new_admin }` |

## Integration with fee_calculator

The typical call sequence is:

1. `fee_calculator::calculate_fee(amount)` → returns `fee_amount`
2. `fee_distributor::distribute(settlement_caller, fee_amount)` — splits the fee

See `src/test.rs::test_distribute_fee_calculator_output` for an example.

## Setup

```bash
cd dabdub_contracts
cargo test -p fee_distributor
```
