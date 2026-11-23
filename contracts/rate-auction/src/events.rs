#![allow(deprecated)]

use soroban_sdk::{BytesN, Env, Symbol};

/// Emitted when the contract is initialized.
pub fn initialized(env: &Env, admin: &soroban_sdk::Address) {
    let topics = (Symbol::new(env, "initialized"),);
    env.events().publish(topics, admin.clone());
}

/// Emitted when a new rate auction is started.
///
/// Topics: ("rate_auction_started", invoice_id)
/// Data:   (issuer, start_rate_bps, max_rate_bps, step_bps, step_ledgers, face_value)
pub fn rate_auction_started(
    env: &Env,
    invoice_id: &BytesN<32>,
    issuer: &soroban_sdk::Address,
    start_rate_bps: u32,
    max_rate_bps: u32,
    step_bps: u32,
    step_ledgers: u32,
    face_value: i128,
) {
    let topics = (Symbol::new(env, "rate_auction_started"), invoice_id.clone());
    env.events().publish(
        topics,
        (
            issuer.clone(),
            start_rate_bps,
            max_rate_bps,
            step_bps,
            step_ledgers,
            face_value,
        ),
    );
}

/// Emitted when an investor commits funds at the current rate.
///
/// Topics: ("rate_committed", invoice_id)
/// Data:   (investor, amount, locked_rate_bps, total_committed)
pub fn rate_committed(
    env: &Env,
    invoice_id: &BytesN<32>,
    investor: &soroban_sdk::Address,
    amount: i128,
    locked_rate_bps: u32,
    total_committed: i128,
) {
    let topics = (Symbol::new(env, "rate_committed"), invoice_id.clone());
    env.events().publish(
        topics,
        (investor.clone(), amount, locked_rate_bps, total_committed),
    );
}

/// Emitted when an auction closes (filled, expired, or cancelled).
///
/// Topics: ("rate_auction_closed", invoice_id)
/// Data:   (status_u32, clearing_rate_bps, total_committed, timestamp)
///
/// `status_u32` matches `AuctionStatus` discriminants:
///   0 = Open (never emitted), 1 = Filled, 2 = Expired, 3 = Cancelled
pub fn rate_auction_closed(
    env: &Env,
    invoice_id: &BytesN<32>,
    status_u32: u32,
    clearing_rate_bps: u32,
    total_committed: i128,
) {
    let topics = (Symbol::new(env, "rate_auction_closed"), invoice_id.clone());
    let timestamp = env.ledger().timestamp();
    env.events().publish(
        topics,
        (status_u32, clearing_rate_bps, total_committed, timestamp),
    );
}
