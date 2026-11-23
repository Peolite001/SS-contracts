#![allow(deprecated)]
//! Event definitions for the upgrade proxy contract.

use soroban_sdk::{Address, Env, Symbol};

/// Emit a `LogicUpgraded` event when the logic address changes.
pub fn logic_upgraded(env: &Env, old_logic: &Address, new_logic: &Address) {
    env.events().publish(
        (Symbol::new(env, "LogicUpgraded"),),
        (old_logic.clone(), new_logic.clone()),
    );
}

/// Emit a `UpgradeProposed` event when an admin queues an upgrade.
pub fn upgrade_proposed(env: &Env, new_logic: &Address, eta: u64, proposer: &Address) {
    env.events().publish(
        (Symbol::new(env, "UpgradeProposed"),),
        (new_logic.clone(), eta, proposer.clone()),
    );
}

/// Emit an `UpgradeApproved` event when a signer approves the pending proposal.
pub fn upgrade_approved(env: &Env, signer: &Address, approval_count: u32, threshold: u32) {
    env.events().publish(
        (Symbol::new(env, "UpgradeApproved"),),
        (signer.clone(), approval_count, threshold),
    );
}

/// Emit an `UpgradeCancelled` event when an admin cancels a pending proposal.
pub fn upgrade_cancelled(env: &Env, new_logic: &Address) {
    env.events()
        .publish((Symbol::new(env, "UpgradeCancelled"),), new_logic.clone());
}

/// Emit a `ProxyFrozen` event when the proxy is frozen or unfrozen.
pub fn proxy_frozen(env: &Env, frozen: bool) {
    env.events()
        .publish((Symbol::new(env, "ProxyFrozen"),), frozen);
}

/// Emit an `upgrade_dependency_checked` event.
pub fn upgrade_dependency_checked(
    env: &Env,
    contract_id: &Address,
    version: u32,
    compatible: bool,
) {
    env.events().publish(
        (Symbol::new(env, "upgrade_dependency_checked"),),
        (contract_id.clone(), version, compatible),
    );
}

/// Emit an `upgrade_dependency_failed` event.
pub fn upgrade_dependency_failed(env: &Env, contract_id: &Address, reason: Symbol) {
    env.events().publish(
        (Symbol::new(env, "upgrade_dependency_failed"),),
        (contract_id.clone(), reason),
    );
}
