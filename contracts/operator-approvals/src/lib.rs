//! Operator approvals for delegated payout claims (issue #852).
//!
//! An investor can approve up to 5 operators, each until an expiry ledger.
//! An approved operator can trigger the investor's payout claim with
//! `claim_for`, but the payout is always sent to the investor, never to the
//! operator.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, Map, Symbol,
};
use soroban_sdk::{IntoVal, Val};

#[cfg(test)]
mod test;

const TTL_THRESHOLD: u32 = 120_960;
const TTL_EXTEND: u32 = 1_036_800;
const MAX_OPERATORS: u32 = 5;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    NotOperator = 4,
    TooManyOperators = 5,
    InvalidExpiry = 6,
    InvalidAmount = 7,
    NothingToClaim = 8,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Token,
    Operators(Address),
    Payout(Address, BytesN<32>),
}

#[contract]
pub struct OperatorApprovals;

#[contractimpl]
impl OperatorApprovals {
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

    /// Admin-only: deposit a payout that the investor can claim for an invoice.
    pub fn credit_payout(
        env: Env,
        admin: Address,
        investor: Address,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        admin.require_auth();
        let store = env.storage().instance();
        store.extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        let stored: Address = store.get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        if stored != admin {
            return Err(Error::Unauthorized);
        }
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let this = env.current_contract_address();
        let client = token::Client::new(&env, &token_of(&env)?);
        client.transfer(&admin, &this, &amount);

        let key = DataKey::Payout(investor, invoice_id);
        let current: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        save(&env, &key, &(current + amount));
        Ok(())
    }

    /// Approve `operator` to claim payouts for `investor` until
    /// `expires_ledger` (exclusive).
    pub fn approve_operator(
        env: Env,
        investor: Address,
        operator: Address,
        expires_ledger: u32,
    ) -> Result<(), Error> {
        investor.require_auth();
        if expires_ledger <= env.ledger().sequence() {
            return Err(Error::InvalidExpiry);
        }
        let mut ops = operators(&env, &investor);
        if !ops.contains_key(operator.clone()) && ops.len() >= MAX_OPERATORS {
            return Err(Error::TooManyOperators);
        }
        ops.set(operator.clone(), expires_ledger);
        save(&env, &DataKey::Operators(investor.clone()), &ops);
        let data = (operator, expires_ledger);
        emit(&env, "operator_approved", &investor, data);
        Ok(())
    }

    /// Revoke an operator immediately.
    pub fn revoke_operator(env: Env, investor: Address, operator: Address) {
        investor.require_auth();
        let mut ops = operators(&env, &investor);
        ops.remove(operator.clone());
        save(&env, &DataKey::Operators(investor.clone()), &ops);
        emit(&env, "operator_revoked", &investor, operator);
    }

    /// Whether `operator` is approved for `investor` at the current ledger.
    pub fn is_operator(env: Env, investor: Address, operator: Address) -> bool {
        operators(&env, &investor).contains_key(operator)
    }

    /// The payout the investor can claim for an invoice.
    pub fn claimable(env: Env, investor: Address, invoice_id: BytesN<32>) -> i128 {
        let key = DataKey::Payout(investor, invoice_id);
        env.storage().persistent().get(&key).unwrap_or(0)
    }

    /// The investor claims their own payout.
    pub fn claim(env: Env, investor: Address, invoice_id: BytesN<32>) -> Result<i128, Error> {
        investor.require_auth();
        pay(&env, &investor, &invoice_id)
    }

    /// An approved operator claims the payout on behalf of the investor. The
    /// funds always go to the investor.
    pub fn claim_for(
        env: Env,
        operator: Address,
        investor: Address,
        invoice_id: BytesN<32>,
    ) -> Result<i128, Error> {
        operator.require_auth();
        let approved = Self::is_operator(env.clone(), investor.clone(), operator.clone());
        if !approved {
            return Err(Error::NotOperator);
        }
        let amount = pay(&env, &investor, &invoice_id)?;
        let data = (operator, invoice_id, amount);
        emit(&env, "claimed_for", &investor, data);
        Ok(amount)
    }
}

fn emit<D: IntoVal<Env, Val>>(env: &Env, name: &str, investor: &Address, data: D) {
    let topics = (Symbol::new(env, name), investor.clone());
    env.events().publish(topics, data);
}

fn save<V: IntoVal<Env, Val>>(env: &Env, key: &DataKey, value: &V) {
    let store = env.storage().persistent();
    store.set(key, value);
    store.extend_ttl(key, TTL_THRESHOLD, TTL_EXTEND);
}

fn token_of(env: &Env) -> Result<Address, Error> {
    let store = env.storage().instance();
    store.get(&DataKey::Token).ok_or(Error::NotInitialized)
}

/// The investor's operators, with expired approvals dropped.
fn operators(env: &Env, investor: &Address) -> Map<Address, u32> {
    let key = DataKey::Operators(investor.clone());
    let store = env.storage().persistent();
    let stored: Map<Address, u32> = store.get(&key).unwrap_or_else(|| Map::new(env));
    let now = env.ledger().sequence();
    let mut active = Map::new(env);
    for (operator, expires) in stored.iter() {
        if expires > now {
            active.set(operator, expires);
        }
    }
    active
}

fn pay(env: &Env, investor: &Address, invoice_id: &BytesN<32>) -> Result<i128, Error> {
    let key = DataKey::Payout(investor.clone(), invoice_id.clone());
    let store = env.storage().persistent();
    let amount: i128 = store.get(&key).unwrap_or(0);
    if amount <= 0 {
        return Err(Error::NothingToClaim);
    }
    store.remove(&key);
    let this = env.current_contract_address();
    let client = token::Client::new(env, &token_of(env)?);
    client.transfer(&this, investor, &amount);
    Ok(amount)
}
