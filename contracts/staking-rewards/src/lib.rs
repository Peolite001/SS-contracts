#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, Symbol,
};

pub const EPOCH_DURATION: u64 = 30 * 24 * 60 * 60;

#[derive(Clone)]
#[contracttype]
enum DataKey {
    Admin,
    Invoice(BytesN<32>),
    Pool(BytesN<32>),
    TotalStaked(BytesN<32>),
    Stake(u64),
    NextStake,
    NextEpoch(BytesN<32>),
    ActiveEpoch(BytesN<32>),
    Epoch(BytesN<32>, u64),
    CarryOver(BytesN<32>),
    EpochClaimed(BytesN<32>, u64, u64),
}

#[derive(Clone)]
#[contracttype]
pub struct Invoice {
    pub issuer: Address,
    pub token: Address,
    pub bonus_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct Stake {
    pub wallet: Address,
    pub invoice_id: BytesN<32>,
    pub amount: i128,
    pub started_at: u64,
    pub unlock_at: u64,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpochStatus {
    Active,
    Closed,
}

#[derive(Clone)]
#[contracttype]
pub struct Epoch {
    pub id: u64,
    pub started_at: u64,
    pub ends_at: u64,
    pub stake_snapshot: i128,
    pub stake_cutoff: u64,
    pub reward_pool: i128,
    pub claimed_amount: i128,
    pub status: EpochStatus,
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidAmount = 3,
    InvalidLock = 4,
    InvoiceNotFound = 5,
    StakeNotFound = 6,
    StillLocked = 7,
    InsufficientPool = 8,
    Overflow = 9,
    EpochAlreadyActive = 10,
    NoActiveEpoch = 11,
    EpochNotFound = 12,
    EpochNotEnded = 13,
    EpochNotOpen = 14,
    NotEligible = 15,
    AlreadyClaimed = 16,
    NoClaimableReward = 17,
    Unauthorized = 18,
    StakeAccountingMismatch = 19,
    InvoiceAlreadyRegistered = 20,
}

fn load_epoch(env: &Env, invoice_id: &BytesN<32>, epoch_id: u64) -> Result<Epoch, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Epoch(invoice_id.clone(), epoch_id))
        .ok_or(Error::EpochNotFound)
}

fn claimable_for_stake(
    env: &Env,
    invoice_id: &BytesN<32>,
    stake_id: u64,
    stake: &Stake,
    epoch: &Epoch,
) -> Result<i128, Error> {
    if epoch.status != EpochStatus::Active
        || env.ledger().timestamp() >= epoch.ends_at
        || stake_id >= epoch.stake_cutoff
        || stake.invoice_id != invoice_id.clone()
        || epoch.stake_snapshot <= 0
    {
        return Ok(0);
    }

    if env.storage().persistent().has(&DataKey::EpochClaimed(
        invoice_id.clone(),
        epoch.id,
        stake_id,
    )) {
        return Ok(0);
    }

    epoch
        .reward_pool
        .checked_mul(stake.amount)
        .ok_or(Error::Overflow)?
        .checked_div(epoch.stake_snapshot)
        .ok_or(Error::Overflow)
}

#[contract]
pub struct StakingRewards;

#[contractimpl]
impl StakingRewards {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::NextStake, &0u64);
        Ok(())
    }

    pub fn register_invoice(
        env: Env,
        invoice_id: BytesN<32>,
        invoice: Invoice,
    ) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        if env
            .storage()
            .persistent()
            .has(&DataKey::Invoice(invoice_id.clone()))
        {
            return Err(Error::InvoiceAlreadyRegistered);
        }
        if invoice.bonus_bps > 10_000 {
            return Err(Error::InvalidAmount);
        }
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Ok(())
    }

    pub fn fund_staking_pool(env: Env, invoice_id: BytesN<32>, amount: i128) -> Result<(), Error> {
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&DataKey::Invoice(invoice_id.clone()))
            .ok_or(Error::InvoiceNotFound)?;
        invoice.issuer.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let pool: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id.clone()))
            .unwrap_or(0);
        let new_pool = pool.checked_add(amount).ok_or(Error::Overflow)?;
        token::Client::new(&env, &invoice.token).transfer(
            &invoice.issuer,
            &env.current_contract_address(),
            &amount,
        );
        env.storage()
            .persistent()
            .set(&DataKey::Pool(invoice_id), &new_pool);
        Ok(())
    }

    pub fn stake_fractions(
        env: Env,
        wallet: Address,
        invoice_id: BytesN<32>,
        amount: i128,
        lock_period: u64,
    ) -> Result<u64, Error> {
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&DataKey::Invoice(invoice_id.clone()))
            .ok_or(Error::InvoiceNotFound)?;
        wallet.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if lock_period == 0 {
            return Err(Error::InvalidLock);
        }
        let now = env.ledger().timestamp();
        let unlock_at = now.checked_add(lock_period).ok_or(Error::Overflow)?;
        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextStake)
            .unwrap_or(0);
        let next_id = id.checked_add(1).ok_or(Error::Overflow)?;
        let total_staked: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::TotalStaked(invoice_id.clone()))
            .unwrap_or(0);
        let new_total = total_staked.checked_add(amount).ok_or(Error::Overflow)?;

        token::Client::new(&env, &invoice.token).transfer(
            &wallet,
            &env.current_contract_address(),
            &amount,
        );
        env.storage().instance().set(&DataKey::NextStake, &next_id);
        env.storage().persistent().set(
            &DataKey::Stake(id),
            &Stake {
                wallet,
                invoice_id: invoice_id.clone(),
                amount,
                started_at: now,
                unlock_at,
            },
        );
        env.storage()
            .persistent()
            .set(&DataKey::TotalStaked(invoice_id.clone()), &new_total);
        env.events().publish(
            (Symbol::new(&env, "StakeCreated"), id),
            (invoice_id, amount, unlock_at),
        );
        Ok(id)
    }

    /// Begin an epoch with a snapshot of currently staked tokens and funded rewards.
    pub fn start_epoch(env: Env, invoice_id: BytesN<32>) -> Result<Epoch, Error> {
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&DataKey::Invoice(invoice_id.clone()))
            .ok_or(Error::InvoiceNotFound)?;
        invoice.issuer.require_auth();
        if env
            .storage()
            .persistent()
            .has(&DataKey::ActiveEpoch(invoice_id.clone()))
        {
            return Err(Error::EpochAlreadyActive);
        }

        let epoch_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::NextEpoch(invoice_id.clone()))
            .unwrap_or(0);
        let next_epoch = epoch_id.checked_add(1).ok_or(Error::Overflow)?;
        let started_at = env.ledger().timestamp();
        let ends_at = started_at
            .checked_add(EPOCH_DURATION)
            .ok_or(Error::Overflow)?;
        let stake_snapshot: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::TotalStaked(invoice_id.clone()))
            .unwrap_or(0);
        let stake_cutoff: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextStake)
            .unwrap_or(0);
        let pool: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id.clone()))
            .unwrap_or(0);
        let carry_over: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::CarryOver(invoice_id.clone()))
            .unwrap_or(0);
        let reward_pool = pool.checked_add(carry_over).ok_or(Error::Overflow)?;
        let epoch = Epoch {
            id: epoch_id,
            started_at,
            ends_at,
            stake_snapshot,
            stake_cutoff,
            reward_pool,
            claimed_amount: 0,
            status: EpochStatus::Active,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Pool(invoice_id.clone()), &0i128);
        env.storage()
            .persistent()
            .set(&DataKey::CarryOver(invoice_id.clone()), &0i128);
        env.storage()
            .persistent()
            .set(&DataKey::NextEpoch(invoice_id.clone()), &next_epoch);
        env.storage()
            .persistent()
            .set(&DataKey::ActiveEpoch(invoice_id.clone()), &epoch_id);
        env.storage()
            .persistent()
            .set(&DataKey::Epoch(invoice_id.clone(), epoch_id), &epoch);
        env.events().publish(
            (
                Symbol::new(&env, "staking_epoch_started"),
                invoice_id,
                epoch_id,
            ),
            (
                epoch.started_at,
                epoch.ends_at,
                epoch.stake_snapshot,
                epoch.reward_pool,
            ),
        );
        Ok(epoch)
    }

    /// Close an ended epoch. Its unpaid balance is carried into the next epoch.
    pub fn close_epoch(env: Env, invoice_id: BytesN<32>) -> Result<i128, Error> {
        let epoch_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::ActiveEpoch(invoice_id.clone()))
            .ok_or(Error::NoActiveEpoch)?;
        let mut epoch = load_epoch(&env, &invoice_id, epoch_id)?;
        if env.ledger().timestamp() < epoch.ends_at {
            return Err(Error::EpochNotEnded);
        }

        let carry_over = epoch
            .reward_pool
            .checked_sub(epoch.claimed_amount)
            .ok_or(Error::Overflow)?;
        epoch.status = EpochStatus::Closed;
        env.storage()
            .persistent()
            .set(&DataKey::Epoch(invoice_id.clone(), epoch_id), &epoch);
        env.storage()
            .persistent()
            .set(&DataKey::CarryOver(invoice_id.clone()), &carry_over);
        env.storage()
            .persistent()
            .remove(&DataKey::ActiveEpoch(invoice_id.clone()));
        env.events().publish(
            (
                Symbol::new(&env, "staking_epoch_closed"),
                invoice_id,
                epoch_id,
            ),
            (epoch.claimed_amount, carry_over, env.ledger().timestamp()),
        );
        Ok(carry_over)
    }

    /// Return the stored state and immutable snapshots for an epoch.
    pub fn epoch_status(env: Env, invoice_id: BytesN<32>, epoch_id: u64) -> Result<Epoch, Error> {
        load_epoch(&env, &invoice_id, epoch_id)
    }

    /// Return the amount a stake can claim before its active epoch ends.
    pub fn claimable_amount(env: Env, stake_id: u64, epoch_id: u64) -> Result<i128, Error> {
        let stake: Stake = env
            .storage()
            .persistent()
            .get(&DataKey::Stake(stake_id))
            .ok_or(Error::StakeNotFound)?;
        let epoch = load_epoch(&env, &stake.invoice_id, epoch_id)?;
        claimable_for_stake(&env, &stake.invoice_id, stake_id, &stake, &epoch)
    }

    /// Claim this stake's proportional share of an open epoch's snapshotted pool.
    pub fn claim_rewards(
        env: Env,
        wallet: Address,
        stake_id: u64,
        epoch_id: u64,
    ) -> Result<i128, Error> {
        let stake: Stake = env
            .storage()
            .persistent()
            .get(&DataKey::Stake(stake_id))
            .ok_or(Error::StakeNotFound)?;
        wallet.require_auth();
        if wallet != stake.wallet {
            return Err(Error::Unauthorized);
        }
        let mut epoch = load_epoch(&env, &stake.invoice_id, epoch_id)?;
        let active_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::ActiveEpoch(stake.invoice_id.clone()))
            .ok_or(Error::EpochNotOpen)?;
        if active_id != epoch_id {
            return Err(Error::EpochNotOpen);
        }
        if epoch.status != EpochStatus::Active || env.ledger().timestamp() >= epoch.ends_at {
            return Err(Error::EpochNotOpen);
        }
        if stake_id >= epoch.stake_cutoff {
            return Err(Error::NotEligible);
        }
        let claim_key = DataKey::EpochClaimed(stake.invoice_id.clone(), epoch_id, stake_id);
        if env.storage().persistent().has(&claim_key) {
            return Err(Error::AlreadyClaimed);
        }
        let reward = claimable_for_stake(&env, &stake.invoice_id, stake_id, &stake, &epoch)?;
        if reward <= 0 {
            return Err(Error::NoClaimableReward);
        }
        epoch.claimed_amount = epoch
            .claimed_amount
            .checked_add(reward)
            .ok_or(Error::Overflow)?;
        env.storage().persistent().set(&claim_key, &true);
        env.storage()
            .persistent()
            .set(&DataKey::Epoch(stake.invoice_id.clone(), epoch_id), &epoch);
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&DataKey::Invoice(stake.invoice_id.clone()))
            .ok_or(Error::InvoiceNotFound)?;
        token::Client::new(&env, &invoice.token).transfer(
            &env.current_contract_address(),
            &wallet,
            &reward,
        );
        env.events().publish(
            (
                Symbol::new(&env, "staking_reward_claimed"),
                stake_id,
                epoch_id,
            ),
            (wallet, reward),
        );
        Ok(reward)
    }

    pub fn get_stake(env: Env, stake_id: u64) -> Result<Stake, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Stake(stake_id))
            .ok_or(Error::StakeNotFound)
    }

    pub fn unstake_fractions(env: Env, stake_id: u64) -> Result<i128, Error> {
        let stake: Stake = Self::get_stake(env.clone(), stake_id)?;
        if env.ledger().timestamp() < stake.unlock_at {
            return Err(Error::StillLocked);
        }
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&DataKey::Invoice(stake.invoice_id.clone()))
            .ok_or(Error::InvoiceNotFound)?;
        let mut reward = 0i128;
        if let Some(epoch_id) = env
            .storage()
            .persistent()
            .get::<_, u64>(&DataKey::ActiveEpoch(stake.invoice_id.clone()))
        {
            let mut epoch = load_epoch(&env, &stake.invoice_id, epoch_id)?;
            if env.ledger().timestamp() < epoch.ends_at {
                let claim_key = DataKey::EpochClaimed(stake.invoice_id.clone(), epoch_id, stake_id);
                if !env.storage().persistent().has(&claim_key) {
                    reward =
                        claimable_for_stake(&env, &stake.invoice_id, stake_id, &stake, &epoch)?;
                    if reward > 0 {
                        epoch.claimed_amount = epoch
                            .claimed_amount
                            .checked_add(reward)
                            .ok_or(Error::Overflow)?;
                        env.storage().persistent().set(&claim_key, &true);
                        env.storage()
                            .persistent()
                            .set(&DataKey::Epoch(stake.invoice_id.clone(), epoch_id), &epoch);
                    }
                }
            }
        }

        let total_staked: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::TotalStaked(stake.invoice_id.clone()))
            .unwrap_or(0);
        let new_total = total_staked
            .checked_sub(stake.amount)
            .filter(|total| *total >= 0)
            .ok_or(Error::StakeAccountingMismatch)?;
        let total = stake.amount.checked_add(reward).ok_or(Error::Overflow)?;
        token::Client::new(&env, &invoice.token).transfer(
            &env.current_contract_address(),
            &stake.wallet,
            &total,
        );
        env.storage()
            .persistent()
            .set(&DataKey::TotalStaked(stake.invoice_id.clone()), &new_total);
        env.storage().persistent().remove(&DataKey::Stake(stake_id));
        env.events().publish(
            (Symbol::new(&env, "StakeReleased"), stake_id),
            (stake.amount, reward),
        );
        Ok(reward)
    }
}

#[cfg(test)]
mod test;
