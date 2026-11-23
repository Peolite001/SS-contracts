//! Emergency escape hatch (issue #853).
//!
//! When the protocol has been paused for longer than `ESCAPE_DELAY_LEDGERS`,
//! investors can recover their pro-rata share of an invoice vault. The share
//! is computed against the vault balance at the first escape withdrawal for
//! that invoice, is capped at the investor's principal (no fees, bonuses or
//! yield), and can be withdrawn once per position. Listing and funding are
//! disabled while the protocol is paused, which includes escape mode.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, Symbol,
};
use soroban_sdk::{IntoVal, Val};

#[cfg(test)]
mod test;

const TTL_THRESHOLD: u32 = 120_960;
const TTL_EXTEND: u32 = 1_036_800;
/// Ledgers the protocol must stay paused before escape opens (~3 days).
pub const ESCAPE_DELAY_LEDGERS: u32 = 51_840;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    EscapeNotEnabled = 5,
    AlreadyWithdrawn = 6,
    NoPosition = 7,
    InvalidAmount = 8,
    InvoiceExists = 9,
    InvoiceNotFound = 10,
    InsufficientVault = 11,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Token,
    PausedSince,
    Listed(BytesN<32>),
    Vault(BytesN<32>),
    Total(BytesN<32>),
    Snapshot(BytesN<32>),
    Principal(Address, BytesN<32>),
    Withdrawn(Address, BytesN<32>),
}

#[contract]
pub struct EscapeHatch;

#[contractimpl]
impl EscapeHatch {
    pub fn initialize(env: Env, admin: Address, token: Address) -> Result<(), Error> {
        admin.require_auth();
        let store = env.storage().instance();
        if store.has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        store.set(&DataKey::Admin, &admin);
        store.set(&DataKey::Token, &token);
        store.extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        Ok(())
    }

    /// Admin-only: pause the protocol, starting the escape timer.
    pub fn pause(env: Env, admin: Address) -> Result<(), Error> {
        require_admin(&env, &admin)?;
        let store = env.storage().instance();
        if !store.has(&DataKey::PausedSince) {
            store.set(&DataKey::PausedSince, &env.ledger().sequence());
        }
        Ok(())
    }

    /// Admin-only: unpause the protocol, clearing the escape timer.
    pub fn unpause(env: Env, admin: Address) -> Result<(), Error> {
        require_admin(&env, &admin)?;
        env.storage().instance().remove(&DataKey::PausedSince);
        Ok(())
    }

    pub fn paused_since(env: Env) -> Option<u32> {
        env.storage().instance().get(&DataKey::PausedSince)
    }

    /// True once the protocol has been paused for `ESCAPE_DELAY_LEDGERS`.
    pub fn escape_enabled(env: Env) -> bool {
        match Self::paused_since(env.clone()) {
            Some(since) => env.ledger().sequence() - since >= ESCAPE_DELAY_LEDGERS,
            None => false,
        }
    }

    /// Admin-only: list an invoice for funding.
    pub fn list_invoice(env: Env, admin: Address, invoice_id: BytesN<32>) -> Result<(), Error> {
        require_admin(&env, &admin)?;
        require_unpaused(&env)?;
        let key = DataKey::Listed(invoice_id);
        if env.storage().persistent().has(&key) {
            return Err(Error::InvoiceExists);
        }
        save(&env, &key, &true);
        Ok(())
    }

    /// Fund a listed invoice, adding to the investor's principal.
    pub fn fund(
        env: Env,
        investor: Address,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        investor.require_auth();
        require_unpaused(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let listed = DataKey::Listed(invoice_id.clone());
        if !env.storage().persistent().has(&listed) {
            return Err(Error::InvoiceNotFound);
        }
        let this = env.current_contract_address();
        token_client(&env)?.transfer(&investor, &this, &amount);

        let principal = DataKey::Principal(investor, invoice_id.clone());
        add(&env, &principal, amount);
        add(&env, &DataKey::Total(invoice_id.clone()), amount);
        add(&env, &DataKey::Vault(invoice_id), amount);
        Ok(())
    }

    /// Admin-only: pay funds out of an invoice vault (e.g. to the issuer).
    pub fn disburse(
        env: Env,
        admin: Address,
        invoice_id: BytesN<32>,
        to: Address,
        amount: i128,
    ) -> Result<(), Error> {
        require_admin(&env, &admin)?;
        let key = DataKey::Vault(invoice_id);
        if amount <= 0 || amount > balance(&env, &key) {
            return Err(Error::InsufficientVault);
        }
        add(&env, &key, -amount);
        let this = env.current_contract_address();
        token_client(&env)?.transfer(&this, &to, &amount);
        Ok(())
    }

    pub fn vault_balance(env: Env, invoice_id: BytesN<32>) -> i128 {
        balance(&env, &DataKey::Vault(invoice_id))
    }

    /// Withdraw the investor's pro-rata principal from an invoice vault once
    /// escape mode is enabled. Each position can be withdrawn once.
    pub fn escape_withdraw(
        env: Env,
        investor: Address,
        invoice_id: BytesN<32>,
    ) -> Result<i128, Error> {
        investor.require_auth();
        if !Self::escape_enabled(env.clone()) {
            return Err(Error::EscapeNotEnabled);
        }
        let done = DataKey::Withdrawn(investor.clone(), invoice_id.clone());
        if env.storage().persistent().has(&done) {
            return Err(Error::AlreadyWithdrawn);
        }
        let principal_key = DataKey::Principal(investor.clone(), invoice_id.clone());
        let principal = balance(&env, &principal_key);
        if principal <= 0 {
            return Err(Error::NoPosition);
        }
        let vault_key = DataKey::Vault(invoice_id.clone());
        let vault = balance(&env, &vault_key);
        let snapshot_key = DataKey::Snapshot(invoice_id.clone());
        if !env.storage().persistent().has(&snapshot_key) {
            save(&env, &snapshot_key, &vault);
        }
        let snapshot = balance(&env, &snapshot_key);
        let total = balance(&env, &DataKey::Total(invoice_id.clone()));

        let share = (snapshot * principal / total).min(principal).min(vault);
        save(&env, &done, &true);
        add(&env, &vault_key, -share);
        if share > 0 {
            let this = env.current_contract_address();
            token_client(&env)?.transfer(&this, &investor, &share);
        }
        let topics = (Symbol::new(&env, "escape_withdrawn"), investor);
        env.events().publish(topics, (invoice_id, share));
        Ok(share)
    }
}

fn require_admin(env: &Env, admin: &Address) -> Result<(), Error> {
    admin.require_auth();
    let store = env.storage().instance();
    store.extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
    let stored: Address = store.get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
    if stored != *admin {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

fn require_unpaused(env: &Env) -> Result<(), Error> {
    if env.storage().instance().has(&DataKey::PausedSince) {
        return Err(Error::Paused);
    }
    Ok(())
}

fn token_client(env: &Env) -> Result<token::Client<'_>, Error> {
    let store = env.storage().instance();
    let token: Address = store.get(&DataKey::Token).ok_or(Error::NotInitialized)?;
    Ok(token::Client::new(env, &token))
}

fn balance(env: &Env, key: &DataKey) -> i128 {
    env.storage().persistent().get(key).unwrap_or(0)
}

fn add(env: &Env, key: &DataKey, delta: i128) {
    let next = balance(env, key) + delta;
    save(env, key, &next);
}

fn save<V: IntoVal<Env, Val>>(env: &Env, key: &DataKey, value: &V) {
    let store = env.storage().persistent();
    store.set(key, value);
    store.extend_ttl(key, TTL_THRESHOLD, TTL_EXTEND);
}
