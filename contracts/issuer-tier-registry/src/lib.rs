//! Issuer tier registry with funding limits derived from settlement history.
//!
//! The escrow reports each invoice outcome. Issuers start at `Bronze`, move
//! up one tier when their settled count reaches the admin-configured
//! threshold, and drop one tier immediately on a default. After a default,
//! `recovery_settlements` clean settlements in a row are needed before the
//! next promotion. The escrow calls `register_invoice` at invoice creation to
//! enforce the tier's `max_invoice_value` and `max_open_exposure`.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol,
};

const TTL_THRESHOLD: u32 = 120_960;
const TTL_EXTEND_TO: u32 = 1_036_800;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInit = 1,
    NotInit = 2,
    Unauthorized = 3,
    InvalidConfig = 4,
    LimitsNotSet = 5,
    TierLimitExceeded = 6,
    InvoiceExists = 7,
    InvalidAmount = 8,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Tier {
    Bronze = 0,
    Silver = 1,
    Gold = 2,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Outcome {
    Settled = 0,
    Late = 1,
    Defaulted = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Escrow,
    Thresholds,
    Limits(Tier),
    History(Address),
    Invoice(BytesN<32>),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Thresholds {
    pub silver_settlements: u32,
    pub gold_settlements: u32,
    pub recovery_settlements: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TierLimits {
    pub max_invoice_value: i128,
    pub max_open_exposure: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct History {
    pub settled_count: u32,
    pub late_count: u32,
    pub default_count: u32,
    pub clean_streak: u32,
    pub tier: Tier,
    pub open_exposure: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenInvoice {
    pub issuer: Address,
    pub value: i128,
}

fn require_stored(env: &Env, key: &DataKey, caller: &Address) -> Result<(), Error> {
    caller.require_auth();
    let stored: Option<Address> = env.storage().instance().get(key);
    if stored.ok_or(Error::NotInit)? != *caller {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

fn load_thresholds(env: &Env) -> Result<Thresholds, Error> {
    let key = DataKey::Thresholds;
    let thresholds: Option<Thresholds> = env.storage().instance().get(&key);
    thresholds.ok_or(Error::NotInit)
}

fn load_limits(env: &Env, tier: Tier) -> Result<TierLimits, Error> {
    let key = DataKey::Limits(tier);
    let limits: Option<TierLimits> = env.storage().instance().get(&key);
    limits.ok_or(Error::LimitsNotSet)
}

fn history_of(env: &Env, issuer: &Address) -> History {
    let key = DataKey::History(issuer.clone());
    let empty = History {
        settled_count: 0,
        late_count: 0,
        default_count: 0,
        clean_streak: 0,
        tier: Tier::Bronze,
        open_exposure: 0,
    };
    env.storage().persistent().get(&key).unwrap_or(empty)
}

fn save_history(env: &Env, issuer: &Address, history: &History) {
    let key = DataKey::History(issuer.clone());
    let store = env.storage().persistent();
    store.set(&key, history);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn earned_tier(thresholds: &Thresholds, settled: u32) -> Tier {
    if settled >= thresholds.gold_settlements {
        Tier::Gold
    } else if settled >= thresholds.silver_settlements {
        Tier::Silver
    } else {
        Tier::Bronze
    }
}

fn can_promote(history: &History, thresholds: &Thresholds) -> bool {
    let earned = earned_tier(thresholds, history.settled_count) as u32;
    let current = history.tier as u32;
    let never_defaulted = history.default_count == 0;
    let recovered = history.clean_streak >= thresholds.recovery_settlements;
    earned > current && (never_defaulted || recovered)
}

fn tier_up(tier: Tier) -> Tier {
    match tier {
        Tier::Bronze => Tier::Silver,
        _ => Tier::Gold,
    }
}

fn tier_down(tier: Tier) -> Tier {
    match tier {
        Tier::Gold => Tier::Silver,
        _ => Tier::Bronze,
    }
}

#[contract]
pub struct IssuerTierRegistry;

#[contractimpl]
impl IssuerTierRegistry {
    /// `escrow` is the only address allowed to register invoices and record
    /// outcomes. Default thresholds: Silver at 3 settlements, Gold at 10,
    /// 2 clean settlements to recover after a default.
    pub fn initialize(env: Env, admin: Address, escrow: Address) -> Result<(), Error> {
        let store = env.storage().instance();
        if store.has(&DataKey::Admin) {
            return Err(Error::AlreadyInit);
        }
        admin.require_auth();
        let thresholds = Thresholds {
            silver_settlements: 3,
            gold_settlements: 10,
            recovery_settlements: 2,
        };
        store.set(&DataKey::Admin, &admin);
        store.set(&DataKey::Escrow, &escrow);
        store.set(&DataKey::Thresholds, &thresholds);
        Ok(())
    }

    pub fn set_thresholds(
        env: Env,
        admin: Address,
        silver_settlements: u32,
        gold_settlements: u32,
        recovery_settlements: u32,
    ) -> Result<(), Error> {
        require_stored(&env, &DataKey::Admin, &admin)?;
        if silver_settlements == 0 || gold_settlements <= silver_settlements {
            return Err(Error::InvalidConfig);
        }
        let thresholds = Thresholds {
            silver_settlements,
            gold_settlements,
            recovery_settlements,
        };
        let key = DataKey::Thresholds;
        env.storage().instance().set(&key, &thresholds);
        Ok(())
    }

    pub fn set_limits(
        env: Env,
        admin: Address,
        tier: Tier,
        max_invoice_value: i128,
        max_open_exposure: i128,
    ) -> Result<(), Error> {
        require_stored(&env, &DataKey::Admin, &admin)?;
        if max_invoice_value <= 0 || max_open_exposure < max_invoice_value {
            return Err(Error::InvalidConfig);
        }
        let limits = TierLimits {
            max_invoice_value,
            max_open_exposure,
        };
        let key = DataKey::Limits(tier);
        env.storage().instance().set(&key, &limits);
        Ok(())
    }

    /// Escrow: called at invoice creation. Reverts with `TierLimitExceeded`
    /// when the invoice is above the issuer's tier limits.
    pub fn register_invoice(
        env: Env,
        caller: Address,
        issuer: Address,
        invoice_id: BytesN<32>,
        value: i128,
    ) -> Result<(), Error> {
        require_stored(&env, &DataKey::Escrow, &caller)?;
        if value <= 0 {
            return Err(Error::InvalidAmount);
        }
        let key = DataKey::Invoice(invoice_id);
        if env.storage().persistent().has(&key) {
            return Err(Error::InvoiceExists);
        }
        let mut history = history_of(&env, &issuer);
        let limits = load_limits(&env, history.tier)?;
        let exposure = history.open_exposure + value;
        if value > limits.max_invoice_value || exposure > limits.max_open_exposure {
            return Err(Error::TierLimitExceeded);
        }
        history.open_exposure = exposure;
        save_history(&env, &issuer, &history);
        let open = OpenInvoice { issuer, value };
        let store = env.storage().persistent();
        store.set(&key, &open);
        store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        Ok(())
    }

    /// Escrow: record the outcome of `invoice_id` and return the issuer's
    /// tier afterwards.
    pub fn record_outcome(
        env: Env,
        caller: Address,
        issuer: Address,
        invoice_id: BytesN<32>,
        outcome: Outcome,
    ) -> Result<Tier, Error> {
        require_stored(&env, &DataKey::Escrow, &caller)?;
        let thresholds = load_thresholds(&env)?;
        let mut history = history_of(&env, &issuer);
        let key = DataKey::Invoice(invoice_id);
        let open: Option<OpenInvoice> = env.storage().persistent().get(&key);
        if let Some(open) = open {
            if open.issuer != issuer {
                return Err(Error::Unauthorized);
            }
            history.open_exposure -= open.value;
            env.storage().persistent().remove(&key);
        }

        let old_tier = history.tier;
        match outcome {
            Outcome::Settled => {
                history.settled_count += 1;
                history.clean_streak += 1;
                if can_promote(&history, &thresholds) {
                    history.tier = tier_up(history.tier);
                    history.clean_streak = 0;
                }
            }
            Outcome::Late => {
                history.late_count += 1;
                history.clean_streak = 0;
            }
            Outcome::Defaulted => {
                history.default_count += 1;
                history.clean_streak = 0;
                history.tier = tier_down(history.tier);
            }
        }
        save_history(&env, &issuer, &history);

        if history.tier != old_tier {
            let topics = (Symbol::new(&env, "tier_changed"), issuer);
            let data = (old_tier, history.tier);
            env.events().publish(topics, data);
        }
        Ok(history.tier)
    }

    pub fn tier_of(env: Env, issuer: Address) -> Tier {
        history_of(&env, &issuer).tier
    }

    pub fn limits_for(env: Env, tier: Tier) -> Result<TierLimits, Error> {
        load_limits(&env, tier)
    }

    pub fn history(env: Env, issuer: Address) -> History {
        history_of(&env, &issuer)
    }
}

#[cfg(test)]
mod test;
