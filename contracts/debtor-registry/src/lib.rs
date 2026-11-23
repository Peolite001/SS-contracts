//! Trusted debtor registry (issue #851).
//!
//! The admin classifies debtors (identified by a hashed id) as `Trusted`,
//! `Neutral`, `Watch` or `Blocked`. Unknown debtors are `Neutral`. Invoices
//! naming a `Blocked` debtor are rejected at creation, and invoices naming a
//! `Watch` debtor cannot be funded until the debtor acknowledges them.
//! `Trusted` and `Neutral` debtors skip the acknowledgement.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol, Vec,
};
use soroban_sdk::{IntoVal, Val};

#[cfg(test)]
mod test;

const TTL_THRESHOLD: u32 = 120_960;
const TTL_EXTEND: u32 = 1_036_800;
const MAX_HISTORY: u32 = 10;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    DebtorBlocked = 4,
    InvoiceExists = 5,
    InvoiceNotFound = 6,
    AcknowledgementRequired = 7,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebtorStatus {
    Trusted,
    Neutral,
    Watch,
    Blocked,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusChange {
    pub status: DebtorStatus,
    pub reason_hash: BytesN<32>,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invoice {
    pub debtor_hash: BytesN<32>,
    pub debtor: Address,
    pub acknowledged: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Status(BytesN<32>),
    History(BytesN<32>),
    Invoice(BytesN<32>),
}

#[contract]
pub struct DebtorRegistry;

#[contractimpl]
impl DebtorRegistry {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        admin.require_auth();
        let store = env.storage().instance();
        if store.has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        store.set(&DataKey::Admin, &admin);
        store.extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        Ok(())
    }

    /// Admin-only: set the status of a debtor, recording the reason hash and
    /// the ledger timestamp. Only the latest 10 changes are kept.
    pub fn set_debtor_status(
        env: Env,
        admin: Address,
        debtor_hash: BytesN<32>,
        status: DebtorStatus,
        reason_hash: BytesN<32>,
    ) -> Result<(), Error> {
        admin.require_auth();
        if admin_of(&env)? != admin {
            return Err(Error::Unauthorized);
        }
        let key = DataKey::Status(debtor_hash.clone());
        save(&env, &key, &status);

        let mut history = Self::history_of(env.clone(), debtor_hash.clone());
        let change = StatusChange {
            status,
            reason_hash: reason_hash.clone(),
            timestamp: env.ledger().timestamp(),
        };
        history.push_back(change);
        if history.len() > MAX_HISTORY {
            history.pop_front();
        }
        save(&env, &DataKey::History(debtor_hash.clone()), &history);

        let topics = (Symbol::new(&env, "debtor_status_changed"), debtor_hash);
        env.events().publish(topics, (status, reason_hash));
        Ok(())
    }

    /// The status of a debtor; unknown debtors are `Neutral`.
    pub fn status_of(env: Env, debtor_hash: BytesN<32>) -> DebtorStatus {
        let store = env.storage().persistent();
        let key = DataKey::Status(debtor_hash);
        store.get(&key).unwrap_or(DebtorStatus::Neutral)
    }

    /// The latest (up to 10) status changes of a debtor, oldest first.
    pub fn history_of(env: Env, debtor_hash: BytesN<32>) -> Vec<StatusChange> {
        let store = env.storage().persistent();
        let key = DataKey::History(debtor_hash);
        store.get(&key).unwrap_or_else(|| Vec::new(&env))
    }

    /// Create an invoice naming a debtor. Reverts for blocked debtors.
    pub fn create_invoice(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
        debtor_hash: BytesN<32>,
        debtor: Address,
    ) -> Result<(), Error> {
        issuer.require_auth();
        admin_of(&env)?;
        let key = DataKey::Invoice(invoice_id);
        if env.storage().persistent().has(&key) {
            return Err(Error::InvoiceExists);
        }
        let status = Self::status_of(env.clone(), debtor_hash.clone());
        if status == DebtorStatus::Blocked {
            return Err(Error::DebtorBlocked);
        }
        let invoice = Invoice {
            debtor_hash,
            debtor,
            acknowledged: false,
        };
        save(&env, &key, &invoice);
        Ok(())
    }

    /// The debtor named on an invoice acknowledges it.
    pub fn acknowledge(env: Env, invoice_id: BytesN<32>) -> Result<(), Error> {
        let key = DataKey::Invoice(invoice_id);
        let mut invoice = invoice_of(&env, &key)?;
        invoice.debtor.require_auth();
        invoice.acknowledged = true;
        save(&env, &key, &invoice);
        Ok(())
    }

    /// Check whether an invoice may be funded under its debtor's current
    /// status: blocked debtors are rejected and watch debtors must have
    /// acknowledged the invoice.
    pub fn check_fundable(env: Env, invoice_id: BytesN<32>) -> Result<(), Error> {
        let key = DataKey::Invoice(invoice_id);
        let invoice = invoice_of(&env, &key)?;
        match Self::status_of(env, invoice.debtor_hash) {
            DebtorStatus::Blocked => Err(Error::DebtorBlocked),
            DebtorStatus::Watch if !invoice.acknowledged => Err(Error::AcknowledgementRequired),
            _ => Ok(()),
        }
    }
}

fn admin_of(env: &Env) -> Result<Address, Error> {
    let store = env.storage().instance();
    store.extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
    store.get(&DataKey::Admin).ok_or(Error::NotInitialized)
}

fn invoice_of(env: &Env, key: &DataKey) -> Result<Invoice, Error> {
    let store = env.storage().persistent();
    store.get(key).ok_or(Error::InvoiceNotFound)
}

fn save<V: IntoVal<Env, Val>>(env: &Env, key: &DataKey, value: &V) {
    let store = env.storage().persistent();
    store.set(key, value);
    store.extend_ttl(key, TTL_THRESHOLD, TTL_EXTEND);
}
