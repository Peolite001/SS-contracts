//! Underwriter staking with premium pricing and first-loss absorption.
//!
//! Underwriters lock stake against an invoice (capped at `max_cover_bps` of
//! face value) for a premium paid by the issuer when funding completes. On a
//! default the stakes absorb the loss pro-rata before investor principal.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, Symbol, Vec,
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
    InvoiceExists = 5,
    InvoiceNotFound = 6,
    CoverExceeded = 7,
    InvoiceNotActive = 8,
    StillActive = 9,
    NoPosition = 10,
    PremiumsPaid = 11,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum InvoiceStatus {
    Active = 0,
    Settled = 1,
    Defaulted = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Config,
    Cover(BytesN<32>),
    Position(BytesN<32>, Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub admin: Address,
    pub token: Address,
    pub max_cover_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cover {
    pub issuer: Address,
    pub face_value: i128,
    pub total_stake: i128,
    pub status: InvoiceStatus,
    pub premiums_paid: bool,
    pub underwriters: Vec<Address>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub stake: i128,
    pub premium_bps: u32,
}

fn load_config(env: &Env) -> Result<Config, Error> {
    let config: Option<Config> = env.storage().instance().get(&DataKey::Config);
    config.ok_or(Error::NotInit)
}

fn require_admin(config: &Config, admin: &Address) -> Result<(), Error> {
    admin.require_auth();
    if *admin != config.admin {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

fn load_cover(env: &Env, invoice_id: &BytesN<32>) -> Result<Cover, Error> {
    let key = DataKey::Cover(invoice_id.clone());
    let cover: Option<Cover> = env.storage().persistent().get(&key);
    cover.ok_or(Error::InvoiceNotFound)
}

fn save_cover(env: &Env, invoice_id: &BytesN<32>, cover: &Cover) {
    let key = DataKey::Cover(invoice_id.clone());
    let store = env.storage().persistent();
    store.set(&key, cover);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn load_position(env: &Env, invoice_id: &BytesN<32>, who: &Address) -> Option<Position> {
    let key = DataKey::Position(invoice_id.clone(), who.clone());
    env.storage().persistent().get(&key)
}

fn save_position(env: &Env, invoice_id: &BytesN<32>, who: &Address, position: &Position) {
    let key = DataKey::Position(invoice_id.clone(), who.clone());
    let store = env.storage().persistent();
    store.set(&key, position);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn cap_of(config: &Config, cover: &Cover) -> i128 {
    cover.face_value * i128::from(config.max_cover_bps) / BPS
}

#[contract]
pub struct UnderwriterStaking;

#[contractimpl]
impl UnderwriterStaking {
    /// `admin` is the escrow/operator that settles invoices and reports
    /// defaults; absorbed losses are paid to it for distribution to investors.
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        max_cover_bps: u32,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(Error::AlreadyInit);
        }
        admin.require_auth();
        if max_cover_bps > 10_000 {
            return Err(Error::InvalidAmount);
        }
        let config = Config {
            admin,
            token,
            max_cover_bps,
        };
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Issuer: make `invoice_id` with `face_value` available for cover.
    pub fn register_invoice(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
        face_value: i128,
    ) -> Result<(), Error> {
        load_config(&env)?;
        issuer.require_auth();
        if face_value <= 0 {
            return Err(Error::InvalidAmount);
        }
        let key = DataKey::Cover(invoice_id.clone());
        if env.storage().persistent().has(&key) {
            return Err(Error::InvoiceExists);
        }
        let cover = Cover {
            issuer,
            face_value,
            total_stake: 0,
            status: InvoiceStatus::Active,
            premiums_paid: false,
            underwriters: Vec::new(&env),
        };
        save_cover(&env, &invoice_id, &cover);
        Ok(())
    }

    /// Underwriter: lock `stake` against `invoice_id` for `premium_bps`.
    pub fn underwrite(
        env: Env,
        underwriter: Address,
        invoice_id: BytesN<32>,
        stake: i128,
        premium_bps: u32,
    ) -> Result<(), Error> {
        underwriter.require_auth();
        if stake <= 0 {
            return Err(Error::InvalidAmount);
        }
        let config = load_config(&env)?;
        let mut cover = load_cover(&env, &invoice_id)?;
        if cover.status != InvoiceStatus::Active {
            return Err(Error::InvoiceNotActive);
        }
        if cover.total_stake + stake > cap_of(&config, &cover) {
            return Err(Error::CoverExceeded);
        }
        let existing = load_position(&env, &invoice_id, &underwriter);
        if existing.is_none() {
            cover.underwriters.push_back(underwriter.clone());
        }
        let zero = Position {
            stake: 0,
            premium_bps,
        };
        let mut position = existing.unwrap_or(zero);
        position.stake += stake;
        position.premium_bps = premium_bps;
        cover.total_stake += stake;
        save_position(&env, &invoice_id, &underwriter, &position);
        save_cover(&env, &invoice_id, &cover);

        let asset = token::Client::new(&env, &config.token);
        let contract = env.current_contract_address();
        asset.transfer(&underwriter, &contract, &stake);
        let topics = (Symbol::new(&env, "underwritten"), invoice_id);
        let data = (underwriter, stake, premium_bps);
        env.events().publish(topics, data);
        Ok(())
    }

    /// Issuer: when funding completes, pay each underwriter its premium
    /// (`stake * premium_bps`) out of the issuer's proceeds.
    pub fn pay_premiums(env: Env, issuer: Address, invoice_id: BytesN<32>) -> Result<(), Error> {
        issuer.require_auth();
        let config = load_config(&env)?;
        let mut cover = load_cover(&env, &invoice_id)?;
        if issuer != cover.issuer {
            return Err(Error::Unauthorized);
        }
        if cover.premiums_paid {
            return Err(Error::PremiumsPaid);
        }
        let asset = token::Client::new(&env, &config.token);
        for underwriter in cover.underwriters.iter() {
            if let Some(p) = load_position(&env, &invoice_id, &underwriter) {
                let premium = p.stake * i128::from(p.premium_bps) / BPS;
                asset.transfer(&issuer, &underwriter, &premium);
            }
        }
        cover.premiums_paid = true;
        save_cover(&env, &invoice_id, &cover);
        Ok(())
    }

    /// Admin: mark the invoice settled so underwriters can release stake.
    pub fn settle(env: Env, admin: Address, invoice_id: BytesN<32>) -> Result<(), Error> {
        let config = load_config(&env)?;
        require_admin(&config, &admin)?;
        let mut cover = load_cover(&env, &invoice_id)?;
        if cover.status != InvoiceStatus::Active {
            return Err(Error::InvoiceNotActive);
        }
        cover.status = InvoiceStatus::Settled;
        save_cover(&env, &invoice_id, &cover);
        Ok(())
    }

    /// Admin: apply a default `loss`. Underwriter stakes absorb it pro-rata
    /// first; returns the part of the loss left for investors.
    pub fn apply_default_loss(
        env: Env,
        admin: Address,
        invoice_id: BytesN<32>,
        loss: i128,
    ) -> Result<i128, Error> {
        let config = load_config(&env)?;
        require_admin(&config, &admin)?;
        if loss < 0 {
            return Err(Error::InvalidAmount);
        }
        let mut cover = load_cover(&env, &invoice_id)?;
        if cover.status != InvoiceStatus::Active {
            return Err(Error::InvoiceNotActive);
        }
        let target = loss.min(cover.total_stake);
        let mut remaining = target;
        let last = cover.underwriters.len().saturating_sub(1);
        for (i, underwriter) in cover.underwriters.iter().enumerate() {
            let mut p = match load_position(&env, &invoice_id, &underwriter) {
                Some(p) => p,
                None => continue,
            };
            let share = if i as u32 == last {
                remaining
            } else {
                target * p.stake / cover.total_stake
            };
            let share = share.min(p.stake).min(remaining);
            p.stake -= share;
            remaining -= share;
            save_position(&env, &invoice_id, &underwriter, &p);
        }
        let absorbed = target - remaining;
        cover.total_stake -= absorbed;
        cover.status = InvoiceStatus::Defaulted;
        save_cover(&env, &invoice_id, &cover);

        if absorbed > 0 {
            let asset = token::Client::new(&env, &config.token);
            let contract = env.current_contract_address();
            asset.transfer(&contract, &admin, &absorbed);
        }
        let topics = (Symbol::new(&env, "loss_absorbed"), invoice_id);
        env.events().publish(topics, absorbed);
        Ok(loss - absorbed)
    }

    /// Underwriter: withdraw what is left of the stake once the invoice is
    /// no longer active.
    pub fn release_stake(
        env: Env,
        underwriter: Address,
        invoice_id: BytesN<32>,
    ) -> Result<i128, Error> {
        underwriter.require_auth();
        let config = load_config(&env)?;
        let mut cover = load_cover(&env, &invoice_id)?;
        if cover.status == InvoiceStatus::Active {
            return Err(Error::StillActive);
        }
        let position = load_position(&env, &invoice_id, &underwriter);
        let mut position = position.ok_or(Error::NoPosition)?;
        let amount = position.stake;
        position.stake = 0;
        cover.total_stake -= amount;
        save_position(&env, &invoice_id, &underwriter, &position);
        save_cover(&env, &invoice_id, &cover);

        if amount > 0 {
            let asset = token::Client::new(&env, &config.token);
            let contract = env.current_contract_address();
            asset.transfer(&contract, &underwriter, &amount);
        }
        let topics = (Symbol::new(&env, "stake_released"), invoice_id);
        env.events().publish(topics, (underwriter, amount));
        Ok(amount)
    }

    /// View: `(staked, remaining_capacity)` for `invoice_id`.
    pub fn cover_of(env: Env, invoice_id: BytesN<32>) -> Result<(i128, i128), Error> {
        let config = load_config(&env)?;
        let cover = load_cover(&env, &invoice_id)?;
        let staked = cover.total_stake;
        Ok((staked, cap_of(&config, &cover) - staked))
    }
}

#[cfg(test)]
mod test;
