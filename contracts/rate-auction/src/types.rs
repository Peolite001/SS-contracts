//! Data types for the rate-auction contract.

use soroban_sdk::{contracttype, Address, BytesN, Vec};

/// Storage key enum for instance and persistent storage.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageKey {
    /// Instance: global admin.
    Admin,
    /// Persistent: auction state keyed by invoice id.
    Auction(BytesN<32>),
    /// Persistent: list of commitment records for an invoice.
    Commitments(BytesN<32>),
}

/// Lifecycle status of an auction.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AuctionStatus {
    /// Accepting commitments; rate is still rising.
    Open = 0,
    /// Fully funded; closed at the clearing rate.
    Filled = 1,
    /// Reached the maximum rate without full funding; all commitments refunded.
    Expired = 2,
    /// Cancelled by the issuer before any commitments were made.
    Cancelled = 3,
}

/// Per-auction state stored in persistent storage.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Auction {
    /// Invoice identifier.
    pub invoice_id: BytesN<32>,
    /// Invoice issuer (the party that started the auction).
    pub issuer: Address,
    /// Payment token used for commitments and refunds.
    pub token: Address,
    /// Total funding target (face value less issuer-side discount).
    pub face_value: i128,
    /// Starting rate in basis points.
    pub start_rate_bps: u32,
    /// Maximum rate in basis points; auction expires if reached unfilled.
    pub max_rate_bps: u32,
    /// Basis-point increment applied every `step_ledgers`.
    pub step_bps: u32,
    /// Number of ledgers between each rate step.
    pub step_ledgers: u32,
    /// Ledger sequence at which the auction was started.
    pub start_ledger: u32,
    /// Total amount committed so far (sum of all `Commitment::amount` fields).
    pub total_committed: i128,
    /// Current lifecycle status.
    pub status: AuctionStatus,
}

impl Auction {
    /// Compute the rate that applies at ledger sequence `now`.
    ///
    /// Rate rises by `step_bps` for every complete `step_ledgers` interval
    /// that has elapsed since `start_ledger`, capped at `max_rate_bps`.
    pub fn rate_at(&self, now: u32) -> u32 {
        let elapsed = now.saturating_sub(self.start_ledger);
        // Number of complete step intervals that have elapsed.
        let steps = elapsed / self.step_ledgers;
        let increase = steps.saturating_mul(self.step_bps);
        self.start_rate_bps
            .saturating_add(increase)
            .min(self.max_rate_bps)
    }

    /// Whether the auction has reached the maximum rate at ledger `now`.
    pub fn at_max_rate(&self, now: u32) -> bool {
        self.rate_at(now) >= self.max_rate_bps
    }
}

/// A single investor commitment to an auction.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commitment {
    /// Investor who committed.
    pub investor: Address,
    /// Amount of tokens committed.
    pub amount: i128,
    /// Rate in basis points locked at commitment time.
    pub locked_rate_bps: u32,
    /// Ledger sequence at which the commitment was made.
    pub committed_at_ledger: u32,
}

/// View returned by `get_auction` for off-chain consumers.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuctionView {
    pub auction: Auction,
    pub commitments: Vec<Commitment>,
    pub current_rate_bps: u32,
}
