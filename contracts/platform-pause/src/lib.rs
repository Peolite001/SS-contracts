//! Emergency pause contract for StellarSettle.
//!
//! Lets the admin halt all escrow investment, fraction transfer, and
//! secondary market activity across the platform in a single transaction.
//! Resuming is deliberately slower than pausing: `resume_platform` queues a
//! 24-hour timelock rather than resuming immediately, giving the community
//! time to observe and react to a resumption decision after an incident.
//! Individual invoices can also be paused independently of the platform-wide
//! switch, for incidents scoped to a single invoice.
//!
//! Protocol-wide emergency freeze propagation: critical contracts can be
//! registered to receive freeze/unfreeze notifications. When the platform
//! is paused, the freeze is propagated to all registered contracts.

#![no_std]

mod errors;
mod events;
mod storage;
mod types;

use soroban_sdk::{contract, contractimpl, Address, BytesN, Env, String, Vec};

use errors::Error;
use storage::{
    get_registered_contracts, register_contract, unregister_contract, MAX_REGISTERED_CONTRACTS,
};
use types::{FreezeStatus, StorageKey};

/// Minimum delay between queuing a resume and it taking effect.
pub const RESUME_TIMELOCK_SECONDS: u64 = 24 * 3_600;

/// Reject the zero address (all-zero 32-byte Ed25519 key).
fn ensure_non_zero_address(env: &Env, address: &Address) -> Result<(), Error> {
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

#[contract]
pub struct PlatformPause;

#[contractimpl]
impl PlatformPause {
    /// Initialize the contract with the admin allowed to pause, queue and
    /// execute resumes, and set per-invoice overrides.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        ensure_non_zero_address(&env, &admin)?;
        if storage::get_admin(&env).is_some() {
            return Err(Error::AlreadyInit);
        }
        storage::set_admin(&env, &admin);
        storage::set_max_registered_contracts(&env, MAX_REGISTERED_CONTRACTS);
        events::initialized(&env, &admin);
        Ok(())
    }

    /// Admin-only: immediately halt all platform investment and transfer
    /// activity. Clears any resume that was already queued, since a fresh
    /// pause supersedes it. Also propagates freeze to all registered contracts.
    pub fn pause_platform(env: Env, admin: Address) -> Result<Vec<FreezeStatus>, Error> {
        let stored_admin = storage::get_admin(&env).ok_or(Error::NotInit)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();
        storage::set_paused(&env, true);
        storage::clear_resume_eta(&env);
        events::platform_paused(&env, &admin);

        // Propagate freeze to all registered contracts
        let contracts = get_registered_contracts(&env);
        let mut results = Vec::new(&env);

        for i in 0..contracts.len() {
            let contract = contracts.get(i).unwrap();
            let status = Self::propagate_freeze(&env, &contract, true);
            results.push_back(status);
        }

        events::freeze_propagation_requested(&env, &admin, contracts.len());

        // Emit individual freeze propagation events
        for i in 0..results.len() {
            let status = results.get(i).unwrap();
            events::freeze_propagated(&env, &status.contract, status.success, status.error);
        }

        Ok(results)
    }

    /// Admin-only: queue a resume, or execute one already queued once its
    /// 24-hour timelock has elapsed.
    ///
    /// The first call while paused with no resume queued starts the
    /// timelock and returns without resuming. A later call (by anyone with
    /// admin rights, not necessarily the same caller) after
    /// `RESUME_TIMELOCK_SECONDS` have elapsed since that call actually
    /// clears the pause. Also propagates unfreeze to all registered contracts.
    pub fn resume_platform(env: Env, admin: Address) -> Result<Vec<FreezeStatus>, Error> {
        let stored_admin = storage::get_admin(&env).ok_or(Error::NotInit)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();

        if !storage::get_paused(&env) {
            return Err(Error::NotPaused);
        }

        match storage::get_resume_eta(&env) {
            None => {
                let eta = env.ledger().timestamp() + RESUME_TIMELOCK_SECONDS;
                storage::set_resume_eta(&env, eta);
                events::resume_queued(&env, eta, &admin);
                Ok(Vec::new(&env))
            }
            Some(eta) => {
                if env.ledger().timestamp() < eta {
                    return Err(Error::TimelockNotElapsed);
                }
                storage::set_paused(&env, false);
                storage::clear_resume_eta(&env);
                events::platform_resumed(&env, &admin);

                // Propagate unfreeze to all registered contracts
                let contracts = get_registered_contracts(&env);
                let mut results = Vec::new(&env);

                for i in 0..contracts.len() {
                    let contract = contracts.get(i).unwrap();
                    let status = Self::propagate_freeze(&env, &contract, false);
                    results.push_back(status);
                }

                events::freeze_propagation_requested(&env, &admin, contracts.len());

                // Emit individual freeze propagation events
                for i in 0..results.len() {
                    let status = results.get(i).unwrap();
                    events::freeze_propagated(&env, &status.contract, status.success, status.error);
                }

                Ok(results)
            }
        }
    }

    /// Admin-only: cancel a queued resume without resuming. Useful if new
    /// information surfaces during the 24h window.
    pub fn cancel_resume(env: Env, admin: Address) -> Result<(), Error> {
        let stored_admin = storage::get_admin(&env).ok_or(Error::NotInit)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();

        if storage::get_resume_eta(&env).is_none() {
            return Err(Error::NoResumeQueued);
        }
        storage::clear_resume_eta(&env);
        Ok(())
    }

    /// View: whether the platform is currently paused.
    pub fn is_paused(env: Env) -> bool {
        storage::get_paused(&env)
    }

    /// View: the unix timestamp a queued resume becomes executable at, or
    /// `None` if no resume is queued.
    pub fn resume_eta(env: Env) -> Option<u64> {
        storage::get_resume_eta(&env)
    }

    /// Admin-only: pause or unpause a single invoice, independent of the
    /// platform-wide pause.
    pub fn set_invoice_paused(
        env: Env,
        admin: Address,
        invoice_id: BytesN<32>,
        paused: bool,
    ) -> Result<(), Error> {
        let stored_admin = storage::get_admin(&env).ok_or(Error::NotInit)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();
        storage::set_invoice_paused(&env, &invoice_id, paused);
        events::invoice_pause_set(&env, &invoice_id, paused);
        Ok(())
    }

    /// View: whether `invoice_id` is paused, independent of the
    /// platform-wide pause. Dependent contracts should treat an invoice as
    /// blocked if either `is_paused()` or this returns true.
    pub fn is_invoice_paused(env: Env, invoice_id: BytesN<32>) -> bool {
        storage::get_invoice_paused(&env, &invoice_id)
    }

    /// View: return the current admin.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        storage::get_admin(&env).ok_or(Error::NotInit)
    }

    /// Admin-only: register a critical contract for freeze propagation.
    /// When the platform is paused/unpaused, the freeze state is propagated
    /// to all registered contracts by calling their `set_paused` function.
    pub fn register_contract(
        env: Env,
        admin: Address,
        contract: Address,
        name: String,
    ) -> Result<(), Error> {
        let stored_admin = storage::get_admin(&env).ok_or(Error::NotInit)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();
        ensure_non_zero_address(&env, &contract)?;

        register_contract(&env, &contract).map_err(|_| Error::ContractAlreadyRegistered)?;

        events::contract_registered(&env, &contract, &name);
        Ok(())
    }

    /// Admin-only: unregister a contract from freeze propagation.
    pub fn unregister_contract(env: Env, admin: Address, contract: Address) -> Result<(), Error> {
        let stored_admin = storage::get_admin(&env).ok_or(Error::NotInit)?;
        if admin != stored_admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();

        unregister_contract(&env, &contract).map_err(|_| Error::ContractNotRegistered)?;

        events::contract_unregistered(&env, &contract);
        Ok(())
    }

    /// View: get all registered contracts for freeze propagation.
    pub fn get_registered_contracts(env: Env) -> Result<Vec<Address>, Error> {
        storage::get_admin(&env).ok_or(Error::NotInit)?;
        Ok(get_registered_contracts(&env))
    }

    /// View: check if a contract is registered.
    pub fn is_contract_registered(env: Env, contract: Address) -> Result<bool, Error> {
        storage::get_admin(&env).ok_or(Error::NotInit)?;
        Ok(storage::is_contract_registered(&env, &contract))
    }

    /// Propagate freeze/unfreeze to a single contract by calling its set_paused function.
    /// This is an internal function that handles the cross-contract call.
    fn propagate_freeze(env: &Env, contract: &Address, paused: bool) -> FreezeStatus {
        // Try to call the contract's set_paused function
        // If the contract doesn't have this function or the call fails,
        // we record the failure but don't panic
        let result = env.try_invoke_contract::<()>(
            contract,
            &String::from_str(env, "set_paused"),
            soroban_sdk::vec![env, paused.into_val(env)],
        );

        let (success, error) = match result {
            Ok(_) => (true, None),
            Err(e) => (false, Some(String::from_str(env, &format!("{:?}", e)))),
        };

        FreezeStatus {
            contract: contract.clone(),
            success,
            error,
        }
    }
}

#[cfg(test)]
mod test;
