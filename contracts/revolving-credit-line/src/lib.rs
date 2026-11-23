//! Revolving credit line for repeat issuers.
//!
//! The admin opens a line with a `limit` and a per-ledger interest rate. The
//! issuer draws against the line per invoice and repays as invoices settle;
//! repaid principal becomes available to draw again.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol,
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
    LineExists = 5,
    LineNotFound = 6,
    LimitExceeded = 7,
    LineFrozen = 8,
    OutstandingBalance = 9,
    DrawNotFound = 10,
    Overflow = 11,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Line(Address),
    Draw(BytesN<32>),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreditLine {
    pub limit: i128,
    pub rate_bps: u32,
    pub drawn: i128,
    pub accrued_interest: i128,
    pub last_accrual_ledger: u32,
    pub frozen: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Draw {
    pub issuer: Address,
    pub outstanding: i128,
}

fn require_admin(env: &Env, admin: &Address) -> Result<(), Error> {
    admin.require_auth();
    let stored: Option<Address> = env.storage().instance().get(&DataKey::Admin);
    if stored.ok_or(Error::NotInit)? != *admin {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

fn load_line(env: &Env, issuer: &Address) -> Result<CreditLine, Error> {
    let key = DataKey::Line(issuer.clone());
    let line: Option<CreditLine> = env.storage().persistent().get(&key);
    line.ok_or(Error::LineNotFound)
}

fn save_line(env: &Env, issuer: &Address, line: &CreditLine) {
    let key = DataKey::Line(issuer.clone());
    let store = env.storage().persistent();
    store.set(&key, line);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn load_draw(env: &Env, invoice_id: &BytesN<32>) -> Option<Draw> {
    let key = DataKey::Draw(invoice_id.clone());
    env.storage().persistent().get(&key)
}

fn save_draw(env: &Env, invoice_id: &BytesN<32>, draw: &Draw) {
    let key = DataKey::Draw(invoice_id.clone());
    let store = env.storage().persistent();
    store.set(&key, draw);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

/// Bring `accrued_interest` up to the current ledger.
fn accrue(env: &Env, line: &mut CreditLine) -> Result<(), Error> {
    let now = env.ledger().sequence();
    let elapsed = i128::from(now.saturating_sub(line.last_accrual_ledger));
    let rate = i128::from(line.rate_bps);
    let scaled = line.drawn.checked_mul(rate).ok_or(Error::Overflow)?;
    let scaled = scaled.checked_mul(elapsed).ok_or(Error::Overflow)?;
    line.accrued_interest += scaled / BPS;
    line.last_accrual_ledger = now;
    Ok(())
}

fn available_of(line: &CreditLine) -> i128 {
    line.limit - line.drawn - line.accrued_interest
}

#[contract]
pub struct RevolvingCreditLine;

#[contractimpl]
impl RevolvingCreditLine {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInit);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        Ok(())
    }

    /// Admin: open a credit line for `issuer`. Interest accrues per ledger
    /// at `rate_bps` of the drawn principal.
    pub fn open_line(
        env: Env,
        admin: Address,
        issuer: Address,
        limit: i128,
        rate_bps: u32,
    ) -> Result<(), Error> {
        require_admin(&env, &admin)?;
        if limit <= 0 {
            return Err(Error::InvalidAmount);
        }
        let key = DataKey::Line(issuer.clone());
        if env.storage().persistent().has(&key) {
            return Err(Error::LineExists);
        }
        let line = CreditLine {
            limit,
            rate_bps,
            drawn: 0,
            accrued_interest: 0,
            last_accrual_ledger: env.ledger().sequence(),
            frozen: false,
        };
        save_line(&env, &issuer, &line);
        let topics = (Symbol::new(&env, "line_opened"), issuer);
        env.events().publish(topics, (limit, rate_bps));
        Ok(())
    }

    /// Issuer: draw `amount` against the line for `invoice_id`.
    pub fn draw(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        issuer.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut line = load_line(&env, &issuer)?;
        if line.frozen {
            return Err(Error::LineFrozen);
        }
        accrue(&env, &mut line)?;
        if amount > available_of(&line) {
            return Err(Error::LimitExceeded);
        }
        let mut draw = match load_draw(&env, &invoice_id) {
            Some(d) if d.issuer == issuer => d,
            Some(_) => return Err(Error::Unauthorized),
            None => Draw {
                issuer: issuer.clone(),
                outstanding: 0,
            },
        };
        line.drawn += amount;
        draw.outstanding += amount;
        save_line(&env, &issuer, &line);
        save_draw(&env, &invoice_id, &draw);
        let topics = (Symbol::new(&env, "drawn"), issuer, invoice_id);
        env.events().publish(topics, amount);
        Ok(())
    }

    /// Repay against the draw for `invoice_id`. Accrued interest is paid
    /// first, then principal; repaid principal frees capacity on the line.
    pub fn repay(
        env: Env,
        payer: Address,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        payer.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut draw = load_draw(&env, &invoice_id).ok_or(Error::DrawNotFound)?;
        let mut line = load_line(&env, &draw.issuer)?;
        accrue(&env, &mut line)?;
        let interest_paid = amount.min(line.accrued_interest);
        let principal_paid = (amount - interest_paid).min(draw.outstanding);
        line.accrued_interest -= interest_paid;
        line.drawn -= principal_paid;
        draw.outstanding -= principal_paid;
        save_line(&env, &draw.issuer, &line);
        save_draw(&env, &invoice_id, &draw);
        let topics = (Symbol::new(&env, "repaid"), draw.issuer, invoice_id);
        env.events()
            .publish(topics, (principal_paid, interest_paid));
        Ok(())
    }

    /// View: `limit - drawn - accrued_interest` at the current ledger.
    pub fn available(env: Env, issuer: Address) -> Result<i128, Error> {
        let mut line = load_line(&env, &issuer)?;
        accrue(&env, &mut line)?;
        Ok(available_of(&line))
    }

    pub fn get_line(env: Env, issuer: Address) -> Result<CreditLine, Error> {
        load_line(&env, &issuer)
    }

    /// Admin: block new draws; repayments are still accepted.
    pub fn freeze_line(env: Env, admin: Address, issuer: Address) -> Result<(), Error> {
        require_admin(&env, &admin)?;
        let mut line = load_line(&env, &issuer)?;
        line.frozen = true;
        save_line(&env, &issuer, &line);
        let topics = (Symbol::new(&env, "line_frozen"), issuer);
        env.events().publish(topics, true);
        Ok(())
    }

    /// Admin: close the line. Reverts while anything is still owed.
    pub fn close_line(env: Env, admin: Address, issuer: Address) -> Result<(), Error> {
        require_admin(&env, &admin)?;
        let mut line = load_line(&env, &issuer)?;
        accrue(&env, &mut line)?;
        if line.drawn != 0 || line.accrued_interest != 0 {
            return Err(Error::OutstandingBalance);
        }
        let key = DataKey::Line(issuer);
        env.storage().persistent().remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod test;
