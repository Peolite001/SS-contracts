//! Descending-rate funding auction contract for StellarSettle.
//!
//! An issuer publishes an invoice with a funding target (`face_value`). The
//! auction starts at a low yield rate that rises by `step_bps` every
//! `step_ledgers` until the invoice is fully funded or the rate reaches
//! `max_rate_bps`.
//!
//! Each investor's yield is locked at the rate in effect when they call
//! `commit_at_current_rate`. The auction closes immediately when
//! `total_committed >= face_value` (filled), or stays open until someone
//! calls `close_expired` once the max rate is reached and the invoice is
//! still unfilled (all commitments are then refunded).
//!
//! # Entry points
//! - `initialize(admin)` — one-time setup
//! - `start_rate_auction(issuer, invoice_id, token, face_value, start_rate_bps, max_rate_bps, step_bps, step_ledgers)`
//! - `current_rate_bps(invoice_id)` — view: rate at current ledger
//! - `commit_at_current_rate(investor, invoice_id, amount)` — place a commitment
//! - `cancel_auction(issuer, invoice_id)` — cancel before any commitments
//! - `close_expired(invoice_id)` — settle an unfilled max-rate auction (refunds)
//! - `get_auction(invoice_id)` — view: full auction state + commitments
//! - `get_commitments(invoice_id)` — view: commitment list

#![no_std]

mod errors;
mod events;
mod storage;
mod types;

pub use types::{Auction, AuctionStatus, AuctionView, Commitment};

use soroban_sdk::{contract, contractimpl, token, Address, BytesN, Env, String};

use errors::Error;

// ── Internal helpers ───────────────────────────────────────────────────────

/// Reject the Stellar zero address.
fn ensure_non_zero(env: &Env, address: &Address) -> Result<(), Error> {
    let zero_str = String::from_str(
        env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    );
    let zero = Address::from_string(&zero_str);
    if *address == zero {
        return Err(Error::InvalidAddress);
    }
    Ok(())
}

/// Load admin or return `NotInit`.
fn require_init(env: &Env) -> Result<Address, Error> {
    storage::get_admin(env).ok_or(Error::NotInit)
}

/// Load an auction or return `AuctionNotFound`.
fn load_auction(env: &Env, invoice_id: &BytesN<32>) -> Result<Auction, Error> {
    storage::get_auction(env, invoice_id).ok_or(Error::AuctionNotFound)
}

/// Require the auction to be in the `Open` state.
fn require_open(auction: &Auction) -> Result<(), Error> {
    if auction.status != AuctionStatus::Open {
        return Err(Error::AuctionNotOpen);
    }
    Ok(())
}

// ── Contract ───────────────────────────────────────────────────────────────

#[contract]
pub struct RateAuction;

#[contractimpl]
impl RateAuction {
    /// Initialize the contract. Must be called exactly once.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if storage::get_admin(&env).is_some() {
            return Err(Error::AlreadyInit);
        }
        ensure_non_zero(&env, &admin)?;
        admin.require_auth();
        storage::set_admin(&env, &admin);
        events::initialized(&env, &admin);
        Ok(())
    }

    /// Issuer-only: open a new rate-discovery auction for `invoice_id`.
    ///
    /// Parameters
    /// - `issuer` — the account that owns the invoice and will receive funds
    ///   if the auction fills.
    /// - `invoice_id` — unique 32-byte identifier for this invoice.
    /// - `token` — SEP-41 token used for commitments and refunds.
    /// - `face_value` — total funding target (must be > 0).
    /// - `start_rate_bps` — opening yield rate in basis points (must be > 0).
    /// - `max_rate_bps` — ceiling rate; auction expires at this rate if unfilled
    ///   (must be ≥ `start_rate_bps`).
    /// - `step_bps` — basis-point increment per interval (must be > 0).
    /// - `step_ledgers` — number of ledgers per interval (must be > 0).
    pub fn start_rate_auction(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
        token: Address,
        face_value: i128,
        start_rate_bps: u32,
        max_rate_bps: u32,
        step_bps: u32,
        step_ledgers: u32,
    ) -> Result<(), Error> {
        require_init(&env)?;
        issuer.require_auth();
        ensure_non_zero(&env, &issuer)?;

        if face_value <= 0 {
            return Err(Error::InvalidFaceValue);
        }
        if start_rate_bps == 0 || start_rate_bps > max_rate_bps {
            return Err(Error::InvalidRateRange);
        }
        if step_bps == 0 {
            return Err(Error::InvalidStep);
        }
        if step_ledgers == 0 {
            return Err(Error::InvalidStepLedgers);
        }
        if storage::has_auction(&env, &invoice_id) {
            return Err(Error::AuctionExists);
        }

        let auction = Auction {
            invoice_id: invoice_id.clone(),
            issuer: issuer.clone(),
            token,
            face_value,
            start_rate_bps,
            max_rate_bps,
            step_bps,
            step_ledgers,
            start_ledger: env.ledger().sequence(),
            total_committed: 0,
            status: AuctionStatus::Open,
        };
        storage::set_auction(&env, &auction);

        events::rate_auction_started(
            &env,
            &invoice_id,
            &issuer,
            start_rate_bps,
            max_rate_bps,
            step_bps,
            step_ledgers,
            face_value,
        );
        Ok(())
    }

    /// View: the yield rate in basis points that applies at the current ledger.
    ///
    /// Returns `Err(AuctionNotFound)` if no auction exists for `invoice_id`.
    /// Returns `Err(AuctionNotOpen)` if the auction is no longer open.
    pub fn current_rate_bps(env: Env, invoice_id: BytesN<32>) -> Result<u32, Error> {
        let auction = load_auction(&env, &invoice_id)?;
        require_open(&auction)?;
        Ok(auction.rate_at(env.ledger().sequence()))
    }

    /// Investor: commit `amount` tokens at the rate in effect right now.
    ///
    /// The tokens are transferred from `investor` to this contract immediately.
    /// The locked rate is the value of `current_rate_bps` at the ledger this
    /// transaction executes in.
    ///
    /// If the commitment fills the remaining capacity exactly or would exceed
    /// it, the amount is capped to the remaining need and the auction is
    /// immediately closed as `Filled`; the issuer receives all committed funds
    /// in a single batch transfer.
    ///
    /// If the auction has already reached `max_rate_bps` and is still unfilled,
    /// new commitments are rejected with `AuctionExpired` — the caller should
    /// invoke `close_expired` instead.
    pub fn commit_at_current_rate(
        env: Env,
        investor: Address,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        investor.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let mut auction = load_auction(&env, &invoice_id)?;
        require_open(&auction)?;

        let now_ledger = env.ledger().sequence();
        let rate = auction.rate_at(now_ledger);

        // Reject new commitments once the max rate has been reached and the
        // auction is still unfilled — the caller must use `close_expired`.
        if rate >= auction.max_rate_bps && auction.total_committed < auction.face_value {
            // Only reject if truly unfilled; a filled auction would have been
            // closed already in a prior call.
            return Err(Error::AuctionExpired);
        }

        // Cap the commitment to the remaining funding needed.
        let remaining = auction
            .face_value
            .checked_sub(auction.total_committed)
            .ok_or(Error::Overflow)?;
        if remaining <= 0 {
            return Err(Error::AuctionNotOpen); // already filled (defensive)
        }
        let actual_amount = amount.min(remaining);

        // Pull tokens from the investor.
        let token_client = token::Client::new(&env, &auction.token);
        token_client.transfer(&investor, &env.current_contract_address(), &actual_amount);

        // Record the commitment.
        let mut commitments = storage::get_commitments(&env, &invoice_id);
        commitments.push_back(Commitment {
            investor: investor.clone(),
            amount: actual_amount,
            locked_rate_bps: rate,
            committed_at_ledger: now_ledger,
        });

        auction.total_committed = auction
            .total_committed
            .checked_add(actual_amount)
            .ok_or(Error::Overflow)?;

        events::rate_committed(
            &env,
            &invoice_id,
            &investor,
            actual_amount,
            rate,
            auction.total_committed,
        );

        // Check whether the auction is now fully funded.
        if auction.total_committed >= auction.face_value {
            // Transfer all committed funds to the issuer in one go.
            token_client.transfer(
                &env.current_contract_address(),
                &auction.issuer,
                &auction.total_committed,
            );
            auction.status = AuctionStatus::Filled;
            storage::set_commitments(&env, &invoice_id, &commitments);
            storage::set_auction(&env, &auction);
            events::rate_auction_closed(
                &env,
                &invoice_id,
                AuctionStatus::Filled as u32,
                rate,
                auction.total_committed,
            );
        } else {
            storage::set_commitments(&env, &invoice_id, &commitments);
            storage::set_auction(&env, &auction);
        }

        Ok(())
    }

    /// Close an unfilled auction that has reached the maximum rate and refund
    /// every investor their committed amount.
    ///
    /// Anyone may call this once the auction's current rate equals `max_rate_bps`
    /// and the invoice remains unfilled, so keepers or the issuer can trigger
    /// clean-up without admin involvement.
    ///
    /// Reverts with `AuctionNotOpen` if the auction is already closed.
    /// Reverts with `AuctionExpired` (re-used as "not yet expired") if the
    /// max rate has not yet been reached and there is still capacity — i.e., the
    /// auction is still normally open and accepting commitments.
    pub fn close_expired(env: Env, invoice_id: BytesN<32>) -> Result<(), Error> {
        let mut auction = load_auction(&env, &invoice_id)?;
        require_open(&auction)?;

        let now_ledger = env.ledger().sequence();
        let rate = auction.rate_at(now_ledger);

        // Only allow closing as expired once the max rate is reached.
        if rate < auction.max_rate_bps {
            return Err(Error::AuctionNotOpen);
        }

        // If by some race the auction is now fully funded, refuse — it should
        // have been closed as Filled by the last `commit_at_current_rate`.
        if auction.total_committed >= auction.face_value {
            return Err(Error::AuctionNotOpen);
        }

        // Refund every investor.
        let commitments = storage::get_commitments(&env, &invoice_id);
        if !commitments.is_empty() {
            let token_client = token::Client::new(&env, &auction.token);
            for commitment in commitments.iter() {
                if commitment.amount > 0 {
                    token_client.transfer(
                        &env.current_contract_address(),
                        &commitment.investor,
                        &commitment.amount,
                    );
                }
            }
        }

        auction.status = AuctionStatus::Expired;
        storage::set_auction(&env, &auction);

        events::rate_auction_closed(
            &env,
            &invoice_id,
            AuctionStatus::Expired as u32,
            rate,
            auction.total_committed,
        );
        Ok(())
    }

    /// Issuer-only: cancel an open auction that has received no commitments.
    ///
    /// Reverts with `HasCommitments` if any investor has already committed.
    /// Reverts with `AuctionNotOpen` if the auction is already closed.
    /// Reverts with `Unauthorized` if `issuer` does not match the stored issuer.
    pub fn cancel_auction(env: Env, issuer: Address, invoice_id: BytesN<32>) -> Result<(), Error> {
        issuer.require_auth();

        let mut auction = load_auction(&env, &invoice_id)?;
        require_open(&auction)?;

        if auction.issuer != issuer {
            return Err(Error::Unauthorized);
        }

        let commitments = storage::get_commitments(&env, &invoice_id);
        if !commitments.is_empty() {
            return Err(Error::HasCommitments);
        }

        auction.status = AuctionStatus::Cancelled;
        storage::set_auction(&env, &auction);

        events::rate_auction_closed(
            &env,
            &invoice_id,
            AuctionStatus::Cancelled as u32,
            auction.rate_at(env.ledger().sequence()),
            0,
        );
        Ok(())
    }

    /// View: the full auction record plus current commitments and rate.
    pub fn get_auction(env: Env, invoice_id: BytesN<32>) -> Result<AuctionView, Error> {
        let auction = load_auction(&env, &invoice_id)?;
        let current_rate_bps = auction.rate_at(env.ledger().sequence());
        let commitments = storage::get_commitments(&env, &invoice_id);
        Ok(AuctionView {
            auction,
            commitments,
            current_rate_bps,
        })
    }

    /// View: the commitment list for an auction.
    pub fn get_commitments(
        env: Env,
        invoice_id: BytesN<32>,
    ) -> Result<soroban_sdk::Vec<Commitment>, Error> {
        load_auction(&env, &invoice_id)?;
        Ok(storage::get_commitments(&env, &invoice_id))
    }

    /// View: the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        require_init(&env)
    }
}

#[cfg(test)]
mod test;
