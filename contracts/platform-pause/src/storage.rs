//! Storage helpers for the platform pause contract.

use soroban_sdk::{Address, BytesN, Env, Vec};

use crate::types::StorageKey;

/// Ledgers below which a persistent entry's TTL is extended (~7 days at 5s/ledger).
pub const TTL_THRESHOLD: u32 = 120_960;
/// Minimum TTL extension in ledger units (~60 days at 5s/ledger).
pub const MIN_TTL_EXTEND: u32 = 1_036_800;

/// Maximum number of contracts that can be registered for freeze propagation.
pub const MAX_REGISTERED_CONTRACTS: u32 = 50;

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&StorageKey::Admin, admin);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);
}

pub fn get_admin(env: &Env) -> Option<Address> {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);
    env.storage().instance().get(&StorageKey::Admin)
}

pub fn set_paused(env: &Env, paused: bool) {
    env.storage().instance().set(&StorageKey::Paused, &paused);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);
}

pub fn get_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&StorageKey::Paused)
        .unwrap_or(false)
}

pub fn set_resume_eta(env: &Env, eta: u64) {
    env.storage().instance().set(&StorageKey::ResumeEta, &eta);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);
}

pub fn get_resume_eta(env: &Env) -> Option<u64> {
    env.storage().instance().get(&StorageKey::ResumeEta)
}

pub fn clear_resume_eta(env: &Env) {
    env.storage().instance().remove(&StorageKey::ResumeEta);
}

pub fn set_invoice_paused(env: &Env, invoice_id: &BytesN<32>, paused: bool) {
    let key = StorageKey::InvoicePaused(invoice_id.clone());
    if paused {
        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, MIN_TTL_EXTEND);
    } else {
        env.storage().persistent().remove(&key);
    }
}

pub fn get_invoice_paused(env: &Env, invoice_id: &BytesN<32>) -> bool {
    let key = StorageKey::InvoicePaused(invoice_id.clone());
    let paused = env.storage().persistent().get(&key).unwrap_or(false);
    if env.storage().persistent().has(&key) {
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, MIN_TTL_EXTEND);
    }
    paused
}

/// Set the maximum number of contracts that can be registered.
pub fn set_max_registered_contracts(env: &Env, max: u32) {
    env.storage()
        .instance()
        .set(&StorageKey::MaxRegisteredContracts, &max);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);
}

/// Check if a contract is registered for freeze propagation.
pub fn is_contract_registered(env: &Env, contract: &Address) -> bool {
    let key = StorageKey::RegisteredContract(contract.clone());
    env.storage().persistent().has(&key)
}

/// Register a contract for freeze propagation.
pub fn register_contract(env: &Env, contract: &Address) -> Result<(), ()> {
    let key = StorageKey::RegisteredContract(contract.clone());
    if env.storage().persistent().has(&key) {
        return Err(());
    }

    let mut contracts: Vec<Address> = env
        .storage()
        .instance()
        .get(&StorageKey::RegisteredContracts)
        .unwrap_or(Vec::new(env));

    let max_contracts = env
        .storage()
        .instance()
        .get(&StorageKey::MaxRegisteredContracts)
        .unwrap_or(MAX_REGISTERED_CONTRACTS);

    if contracts.len() >= max_contracts {
        return Err(());
    }

    contracts.push_back(contract.clone());
    env.storage()
        .instance()
        .set(&StorageKey::RegisteredContracts, &contracts);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);

    env.storage().persistent().set(&key, &true);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, MIN_TTL_EXTEND);

    Ok(())
}

/// Unregister a contract from freeze propagation.
pub fn unregister_contract(env: &Env, contract: &Address) -> Result<(), ()> {
    let key = StorageKey::RegisteredContract(contract.clone());
    if !env.storage().persistent().has(&key) {
        return Err(());
    }

    let mut contracts: Vec<Address> = env
        .storage()
        .instance()
        .get(&StorageKey::RegisteredContracts)
        .unwrap_or(Vec::new(env));

    // Remove the contract from the list
    let mut new_contracts = Vec::new(env);
    for i in 0..contracts.len() {
        let c = contracts.get(i).unwrap();
        if c != *contract {
            new_contracts.push_back(c);
        }
    }

    env.storage()
        .instance()
        .set(&StorageKey::RegisteredContracts, &new_contracts);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);

    env.storage().persistent().remove(&key);

    Ok(())
}

/// Get all registered contracts for freeze propagation.
pub fn get_registered_contracts(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&StorageKey::RegisteredContracts)
        .unwrap_or(Vec::new(env))
}

/// Set the maximum number of contracts that can be registered.
pub fn set_max_registered_contracts(env: &Env, max: u32) {
    env.storage()
        .instance()
        .set(&StorageKey::MaxRegisteredContracts, &max);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, MIN_TTL_EXTEND);
}
