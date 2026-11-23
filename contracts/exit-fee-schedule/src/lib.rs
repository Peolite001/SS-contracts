//! Early exit fee schedule for investors withdrawing from open funding.
//!
//! During the last `window_ledgers` before an invoice's funding deadline the
//! exit fee moves linearly from `start_bps` to `end_bps` (either direction).
//! Collected fees go to the issuer-protection reserve. The fee is waived when
//! the invoice failed to reach its minimum funding by the deadline.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, Symbol,
};

const TTL_THRESHOLD: u32 = 120_960;
const TTL_EXTEND_TO: u32 = 1_036_800;
const BPS: i128 = 10_000;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInit = 1,
    NotInit = 2,
    Unauthorized = 3,
    InvalidAmount = 4,
    FeeTooHigh = 5,
    InvalidWindow = 6,
    InvoiceExists = 7,
    InvoiceNotFound = 8,
    NoCommitment = 9,
    FundingClosed = 10,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Config,
    Schedule,
    Funding(BytesN<32>),
    Commitment(BytesN<32>, Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub admin: Address,
    pub token: Address,
    pub reserve: Address,
    pub max_fee_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeSchedule {
    pub start_bps: u32,
    pub end_bps: u32,
    pub window_ledgers: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Funding {
    pub deadline_ledger: u32,
    pub min_funding: i128,
    pub total_committed: i128,
}

fn load_config(env: &Env) -> Result<Config, Error> {
    let config: Option<Config> = env.storage().instance().get(&DataKey::Config);
    config.ok_or(Error::NotInit)
}

fn load_funding(env: &Env, invoice_id: &BytesN<32>) -> Result<Funding, Error> {
    let key = DataKey::Funding(invoice_id.clone());
    let funding: Option<Funding> = env.storage().persistent().get(&key);
    funding.ok_or(Error::InvoiceNotFound)
}

fn save_funding(env: &Env, invoice_id: &BytesN<32>, funding: &Funding) {
    let key = DataKey::Funding(invoice_id.clone());
    let store = env.storage().persistent();
    store.set(&key, funding);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn commitment_of(env: &Env, invoice_id: &BytesN<32>, investor: &Address) -> i128 {
    let key = DataKey::Commitment(invoice_id.clone(), investor.clone());
    env.storage().persistent().get(&key).unwrap_or(0)
}

fn save_commitment(env: &Env, invoice_id: &BytesN<32>, investor: &Address, amount: i128) {
    let key = DataKey::Commitment(invoice_id.clone(), investor.clone());
    let store = env.storage().persistent();
    store.set(&key, &amount);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

/// Exit fee in basis points for `funding` at the current ledger.
fn fee_bps_at(env: &Env, funding: &Funding) -> u32 {
    let now = env.ledger().sequence();
    let deadline = funding.deadline_ledger;
    if now >= deadline && funding.total_committed < funding.min_funding {
        return 0;
    }
    let schedule: Option<FeeSchedule> = env.storage().instance().get(&DataKey::Schedule);
    let schedule = match schedule {
        Some(s) => s,
        None => return 0,
    };
    let window_start = deadline.saturating_sub(schedule.window_ledgers);
    if now <= window_start {
        return schedule.start_bps;
    }
    if now >= deadline {
        return schedule.end_bps;
    }
    let start = i64::from(schedule.start_bps);
    let end = i64::from(schedule.end_bps);
    let elapsed = i64::from(now - window_start);
    let window = i64::from(deadline - window_start);
    (start + (end - start) * elapsed / window) as u32
}

/// Split `amount` into `(refund, fee)`; the two always add up to `amount`.
fn split(amount: i128, fee_bps: u32) -> (i128, i128) {
    let fee = amount * i128::from(fee_bps) / BPS;
    (amount - fee, fee)
}

#[contract]
pub struct ExitFeeContract;

#[contractimpl]
impl ExitFeeContract {
    /// `reserve` receives collected exit fees. `max_fee_bps` caps any
    /// schedule the admin can configure.
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        reserve: Address,
        max_fee_bps: u32,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(Error::AlreadyInit);
        }
        admin.require_auth();
        if max_fee_bps > 10_000 {
            return Err(Error::FeeTooHigh);
        }
        let config = Config {
            admin,
            token,
            reserve,
            max_fee_bps,
        };
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    pub fn set_exit_fee_schedule(
        env: Env,
        admin: Address,
        start_bps: u32,
        end_bps: u32,
        window_ledgers: u32,
    ) -> Result<(), Error> {
        let config = load_config(&env)?;
        admin.require_auth();
        if admin != config.admin {
            return Err(Error::Unauthorized);
        }
        if start_bps > config.max_fee_bps || end_bps > config.max_fee_bps {
            return Err(Error::FeeTooHigh);
        }
        if window_ledgers == 0 {
            return Err(Error::InvalidWindow);
        }
        let schedule = FeeSchedule {
            start_bps,
            end_bps,
            window_ledgers,
        };
        let key = DataKey::Schedule;
        env.storage().instance().set(&key, &schedule);
        Ok(())
    }

    /// Issuer: open `invoice_id` for funding until `deadline_ledger`.
    pub fn open_invoice(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
        deadline_ledger: u32,
        min_funding: i128,
    ) -> Result<(), Error> {
        load_config(&env)?;
        issuer.require_auth();
        if min_funding <= 0 {
            return Err(Error::InvalidAmount);
        }
        let key = DataKey::Funding(invoice_id.clone());
        if env.storage().persistent().has(&key) {
            return Err(Error::InvoiceExists);
        }
        let funding = Funding {
            deadline_ledger,
            min_funding,
            total_committed: 0,
        };
        save_funding(&env, &invoice_id, &funding);
        Ok(())
    }

    /// Investor: commit `amount` of the funding token to `invoice_id`.
    pub fn commit(
        env: Env,
        investor: Address,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        investor.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let config = load_config(&env)?;
        let mut funding = load_funding(&env, &invoice_id)?;
        if env.ledger().sequence() >= funding.deadline_ledger {
            return Err(Error::FundingClosed);
        }
        let asset = token::Client::new(&env, &config.token);
        let contract = env.current_contract_address();
        asset.transfer(&investor, &contract, &amount);
        let previous = commitment_of(&env, &invoice_id, &investor);
        save_commitment(&env, &invoice_id, &investor, previous + amount);
        funding.total_committed += amount;
        save_funding(&env, &invoice_id, &funding);
        Ok(())
    }

    /// Investor: withdraw the whole commitment, paying the current exit fee.
    /// Returns the refunded amount.
    pub fn withdraw_commitment(
        env: Env,
        investor: Address,
        invoice_id: BytesN<32>,
    ) -> Result<i128, Error> {
        investor.require_auth();
        let config = load_config(&env)?;
        let mut funding = load_funding(&env, &invoice_id)?;
        let amount = commitment_of(&env, &invoice_id, &investor);
        if amount <= 0 {
            return Err(Error::NoCommitment);
        }
        let (refund, fee) = split(amount, fee_bps_at(&env, &funding));
        save_commitment(&env, &invoice_id, &investor, 0);
        funding.total_committed -= amount;
        save_funding(&env, &invoice_id, &funding);

        let asset = token::Client::new(&env, &config.token);
        let contract = env.current_contract_address();
        asset.transfer(&contract, &investor, &refund);
        if fee > 0 {
            asset.transfer(&contract, &config.reserve, &fee);
            let topics = (Symbol::new(&env, "exit_fee_charged"), invoice_id);
            env.events().publish(topics, (investor, fee));
        }
        Ok(refund)
    }

    pub fn exit_fee_bps(env: Env, invoice_id: BytesN<32>) -> Result<u32, Error> {
        let funding = load_funding(&env, &invoice_id)?;
        Ok(fee_bps_at(&env, &funding))
    }

    /// View: `(refund, fee)` the investor would get by withdrawing now.
    pub fn preview_exit(
        env: Env,
        invoice_id: BytesN<32>,
        investor: Address,
    ) -> Result<(i128, i128), Error> {
        let funding = load_funding(&env, &invoice_id)?;
        let amount = commitment_of(&env, &invoice_id, &investor);
        Ok(split(amount, fee_bps_at(&env, &funding)))
    }
}

#[cfg(test)]
mod test;
