//! Settlement priority queue contract for StellarSettle.
//!
//! When settlement proceeds are less than the total owed, this contract
//! determines who is paid and in what order. The admin configures a strict
//! total ordering over the five [`PayeeClass`] variants; each distribution
//! saturates the highest-priority class before anything reaches a lower one.
//!
//! # Key entry points
//! - `initialize(admin)` — one-time setup
//! - `set_priority(admin, order)` — configure (or reconfigure) the payout order
//! - `register_invoice(admin, invoice_id, owed)` — record what each class is owed
//! - `distribute_by_priority(invoice_id, amount)` — perform the distribution
//! - `preview_distribution(invoice_id, amount)` — read-only dry run

#![no_std]

mod errors;
mod events;
mod storage;
mod types;

pub use types::{ClassAllocation, DistributionResult, InvoiceOwed, PayeeClass, PAYEE_CLASS_COUNT};

use soroban_sdk::{contract, contractimpl, Address, BytesN, Env, String, Vec};

use errors::Error;

// ── Internal helpers ───────────────────────────────────────────────────────

/// Reject the Stellar zero address (all-zero Ed25519 key strkey).
fn ensure_non_zero(env: &Env, address: &Address) -> Result<(), Error> {
    let zero_str = String::from_str(
        env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    );
    let zero = Address::from_string(&zero_str);
    if *address == zero {
        return Err(Error::InvalidAddress);
    }
    Ok(())
}

/// Load admin or return `NotInit`.
fn require_init(env: &Env) -> Result<Address, Error> {
    storage::get_admin(env).ok_or(Error::NotInit)
}

/// Validate that `order` contains each [`PayeeClass`] exactly once.
///
/// Two properties are checked:
/// 1. The length must be exactly `PAYEE_CLASS_COUNT`.
/// 2. No discriminant appears twice (checked via a compact bitset).
fn validate_order(order: &Vec<PayeeClass>) -> Result<(), Error> {
    if order.len() != PAYEE_CLASS_COUNT {
        return Err(Error::InvalidOrderLength);
    }
    // Use a u32 as a 5-bit presence mask — one bit per PayeeClass discriminant.
    let mut seen: u32 = 0;
    for class in order.iter() {
        let bit = 1u32 << (class as u32);
        if seen & bit != 0 {
            return Err(Error::DuplicatePayeeClass);
        }
        seen |= bit;
    }
    // All 5 bits (0b11111 = 31) must be set.
    let expected_mask: u32 = (1 << PAYEE_CLASS_COUNT) - 1;
    if seen != expected_mask {
        return Err(Error::MissingPayeeClass);
    }
    Ok(())
}

/// Core allocation algorithm. Pure function; no storage reads/writes.
///
/// Iterates over `order` in priority sequence. For each class:
/// - Pays min(remaining, owed_for_class).
/// - Records shortfall when `remaining < owed_for_class`.
/// - Stops allocating once `remaining` reaches zero.
fn allocate(
    env: &Env,
    order: &Vec<PayeeClass>,
    owed: &InvoiceOwed,
    amount: i128,
) -> Result<DistributionResult, Error> {
    let mut remaining = amount;
    let mut total_paid: i128 = 0;
    let mut allocations: Vec<ClassAllocation> = Vec::new(env);

    for class in order.iter() {
        let class_owed = owed.owed_for(class);
        let paid = if remaining >= class_owed {
            class_owed
        } else {
            remaining
        };
        let shortfall = class_owed.checked_sub(paid).ok_or(Error::Overflow)?;

        allocations.push_back(ClassAllocation {
            class,
            amount: paid,
            shortfall,
        });

        remaining = remaining.checked_sub(paid).ok_or(Error::Overflow)?;
        total_paid = total_paid.checked_add(paid).ok_or(Error::Overflow)?;
    }

    Ok(DistributionResult {
        allocations,
        total_paid,
    })
}

// ── Contract ───────────────────────────────────────────────────────────────

#[contract]
pub struct SettlementPriorityQueue;

#[contractimpl]
impl SettlementPriorityQueue {
    /// Initialize the contract. Must be called exactly once.
    ///
    /// `admin` is the address allowed to call `set_priority` and
    /// `register_invoice`. Reverts with `AlreadyInit` on a second call.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if storage::get_admin(&env).is_some() {
            return Err(Error::AlreadyInit);
        }
        ensure_non_zero(&env, &admin)?;
        admin.require_auth();
        storage::set_admin(&env, &admin);
        events::initialized(&env, &admin);
        Ok(())
    }

    /// Admin-only: configure (or reconfigure) the payout priority order.
    ///
    /// `order` must contain each [`PayeeClass`] exactly once. The position in
    /// the vector determines precedence: index 0 is paid first.
    ///
    /// Reverts with:
    /// - `Unauthorized` if `admin` does not match the stored admin.
    /// - `InvalidOrderLength` if `order.len() != PAYEE_CLASS_COUNT`.
    /// - `DuplicatePayeeClass` if any class appears more than once.
    /// - `MissingPayeeClass` if any class is absent (redundant guard).
    pub fn set_priority(env: Env, admin: Address, order: Vec<PayeeClass>) -> Result<(), Error> {
        let stored_admin = require_init(&env)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();
        validate_order(&order)?;
        storage::set_priority(&env, &order);
        events::priority_set(&env, &admin, &order);
        Ok(())
    }

    /// Admin-only: register what each PayeeClass is owed for a given invoice.
    ///
    /// All five `owed` fields must be non-negative. This call does not transfer
    /// tokens — it records the entitlement on-chain so `distribute_by_priority`
    /// and `preview_distribution` can compute allocations.
    ///
    /// Reverts with `InvoiceExists` if the invoice was already registered.
    pub fn register_invoice(
        env: Env,
        admin: Address,
        invoice_id: BytesN<32>,
        owed: InvoiceOwed,
    ) -> Result<(), Error> {
        let stored_admin = require_init(&env)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();

        if storage::has_invoice(&env, &invoice_id) {
            return Err(Error::InvoiceExists);
        }

        // Validate that no owed value is negative.
        if owed.platform_fee < 0
            || owed.insurance_pool < 0
            || owed.senior_investors < 0
            || owed.junior_investors < 0
            || owed.issuer_residual < 0
        {
            return Err(Error::InvalidOwed);
        }

        storage::set_invoice(&env, &invoice_id, &owed);
        Ok(())
    }

    /// Distribute `amount` among payee classes according to the configured
    /// priority order.
    ///
    /// Higher-priority classes are paid in full before any amount reaches a
    /// lower-priority class. When `amount` is insufficient, the first
    /// under-funded class receives the remainder and all subsequent classes
    /// receive zero (shortfall equals their full entitlement).
    ///
    /// Emits one `class_paid` event per class (even those paid zero), then a
    /// `distribution_complete` event with the total paid.
    ///
    /// Returns a [`DistributionResult`] whose `total_paid` equals `amount`
    /// exactly (the sum of all allocations).
    ///
    /// Reverts with:
    /// - `PriorityNotSet` if `set_priority` was never called.
    /// - `InvoiceNotFound` if `invoice_id` was never registered.
    /// - `InvalidAmount` if `amount <= 0`.
    /// - `AlreadyDistributed` if this invoice has already been distributed.
    pub fn distribute_by_priority(
        env: Env,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<DistributionResult, Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let order = storage::get_priority(&env).ok_or(Error::PriorityNotSet)?;
        let owed = storage::get_invoice(&env, &invoice_id).ok_or(Error::InvoiceNotFound)?;

        // Guard: once an invoice has been distributed (owed zeroed out), reject
        // repeat calls. A fully-distributed invoice has all fields set to 0.
        let total_owed = owed.total().ok_or(Error::Overflow)?;
        if total_owed == 0 {
            return Err(Error::AlreadyDistributed);
        }

        let result = allocate(&env, &order, &owed, amount)?;

        // Emit per-class events.
        for alloc in result.allocations.iter() {
            events::class_paid(&env, &invoice_id, &alloc);
        }
        events::distribution_complete(&env, &invoice_id, result.total_paid);

        // Mark the invoice as distributed by zeroing out the owed record.
        let zeroed = InvoiceOwed {
            platform_fee: 0,
            insurance_pool: 0,
            senior_investors: 0,
            junior_investors: 0,
            issuer_residual: 0,
        };
        storage::set_invoice(&env, &invoice_id, &zeroed);

        Ok(result)
    }

    /// Read-only: compute the distribution allocations for `amount` without
    /// mutating any state or emitting events.
    ///
    /// The result matches what `distribute_by_priority` would produce for the
    /// same inputs, allowing off-chain simulation before committing.
    ///
    /// Reverts with the same errors as `distribute_by_priority`.
    pub fn preview_distribution(
        env: Env,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<DistributionResult, Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let order = storage::get_priority(&env).ok_or(Error::PriorityNotSet)?;
        let owed = storage::get_invoice(&env, &invoice_id).ok_or(Error::InvoiceNotFound)?;
        allocate(&env, &order, &owed, amount)
    }

    /// View: return the current priority order, or `None` if not yet set.
    pub fn get_priority(env: Env) -> Option<Vec<PayeeClass>> {
        storage::get_priority(&env)
    }

    /// View: return the owed amounts registered for `invoice_id`.
    pub fn get_invoice_owed(env: Env, invoice_id: BytesN<32>) -> Result<InvoiceOwed, Error> {
        storage::get_invoice(&env, &invoice_id).ok_or(Error::InvoiceNotFound)
    }

    /// View: return the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        require_init(&env)
    }
}

#[cfg(test)]
mod test;
