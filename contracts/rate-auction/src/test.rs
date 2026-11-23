//! Unit tests for the rate-auction contract.
//!
//! Coverage:
//!   - Initialization and re-init guard
//!   - Rate progression: exact step_bps per interval, never exceeds max
//!   - Each investor's yield uses the rate at their commitment time
//!   - Auction closes immediately when fully funded
//!   - Unfilled auctions refund every commitment
//!   - Cancellation after a commitment reverts
//!   - Invalid parameter combinations revert

#![cfg(test)]

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{Client as TokenClient, StellarAssetClient as AssetClient},
    Address, BytesN, Env,
};

use crate::{AuctionStatus, RateAuction, RateAuctionClient};

// ── Helpers ────────────────────────────────────────────────────────────────

struct Ctx<'a> {
    env: Env,
    admin: Address,
    client: RateAuctionClient<'a>,
}

fn setup() -> Ctx<'static> {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(RateAuction, ());
    let client = RateAuctionClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    Ctx { env, admin, client }
}

fn fresh_id(env: &Env, seed: u8) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    BytesN::from_array(env, &bytes)
}

/// Create a test token; returns (token_address, asset_client_for_minting).
fn create_token(env: &Env) -> (Address, AssetClient<'_>) {
    let token_admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin);
    let asset = AssetClient::new(env, &token_id.address());
    (token_id.address(), asset)
}

/// Advance the ledger sequence by `n` (timestamp advances proportionally).
fn advance_ledgers(env: &Env, n: u32) {
    env.ledger().with_mut(|li| {
        li.sequence_number += n;
        li.timestamp += (n as u64) * 5;
    });
}

// ── Initialization ─────────────────────────────────────────────────────────

#[test]
fn test_initialize_stores_admin() {
    let ctx = setup();
    assert_eq!(ctx.client.get_admin(), ctx.admin);
}

#[test]
fn test_initialize_rejects_reinit() {
    let ctx = setup();
    assert!(ctx.client.try_initialize(&ctx.admin).is_err());
}

// ── Parameter validation ───────────────────────────────────────────────────

#[test]
fn test_start_auction_rejects_zero_face_value() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 1);

    let result =
        ctx.client
            .try_start_rate_auction(&issuer, &id, &token_addr, &0, &100, &500, &10, &5);
    assert!(result.is_err());
}

#[test]
fn test_start_auction_rejects_start_rate_exceeds_max() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 2);

    // start_rate (600) > max_rate (500)
    let result =
        ctx.client
            .try_start_rate_auction(&issuer, &id, &token_addr, &1000, &600, &500, &10, &5);
    assert!(result.is_err());
}

#[test]
fn test_start_auction_rejects_zero_step_bps() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 3);

    let result =
        ctx.client
            .try_start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &0, &5);
    assert!(result.is_err());
}

#[test]
fn test_start_auction_rejects_zero_step_ledgers() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 4);

    let result =
        ctx.client
            .try_start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &10, &0);
    assert!(result.is_err());
}

#[test]
fn test_start_auction_rejects_duplicate_invoice_id() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 5);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &10, &5);
    let result =
        ctx.client
            .try_start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &10, &5);
    assert!(result.is_err());
}

// ── Rate progression ───────────────────────────────────────────────────────

#[test]
fn test_rate_starts_at_start_rate() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 10);

    // start=100 bps, max=500, step=50 bps per 10 ledgers
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);

    // No ledgers advanced — should be at start rate.
    let rate = ctx.client.current_rate_bps(&id);
    assert_eq!(rate, 100);
}

#[test]
fn test_rate_rises_by_exact_step_per_interval() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 11);

    // start=100, max=500, step=50 per 10 ledgers
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);

    // After 10 ledgers → 1 interval → 100 + 50 = 150
    advance_ledgers(&ctx.env, 10);
    assert_eq!(ctx.client.current_rate_bps(&id), 150);

    // After 20 ledgers total → 2 intervals → 100 + 100 = 200
    advance_ledgers(&ctx.env, 10);
    assert_eq!(ctx.client.current_rate_bps(&id), 200);

    // After 30 ledgers total → 3 intervals → 100 + 150 = 250
    advance_ledgers(&ctx.env, 10);
    assert_eq!(ctx.client.current_rate_bps(&id), 250);
}

#[test]
fn test_rate_does_not_exceed_max() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 12);

    // start=400, max=500, step=100 per 1 ledger
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &400, &500, &100, &1);

    // After 1 ledger: 400 + 100 = 500 (at max)
    advance_ledgers(&ctx.env, 1);
    assert_eq!(ctx.client.current_rate_bps(&id), 500);

    // After 2 ledgers: would be 600 without cap, still 500
    advance_ledgers(&ctx.env, 1);
    assert_eq!(ctx.client.current_rate_bps(&id), 500);

    // After 100 more ledgers: still 500
    advance_ledgers(&ctx.env, 100);
    assert_eq!(ctx.client.current_rate_bps(&id), 500);
}

#[test]
fn test_rate_before_first_complete_interval_stays_at_start() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 13);

    // step_ledgers = 10; advancing only 9 is not a complete interval
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);

    advance_ledgers(&ctx.env, 9);
    assert_eq!(ctx.client.current_rate_bps(&id), 100);
}

// ── Commitment locking ─────────────────────────────────────────────────────

#[test]
fn test_commitment_locks_rate_at_commitment_time() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor1 = Address::generate(&ctx.env);
    let investor2 = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 20);

    token.mint(&investor1, &500);
    token.mint(&investor2, &500);

    // start=100, max=500, step=50 per 10 ledgers, face=1000
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);

    // Investor1 commits at ledger 0 → rate = 100
    ctx.client.commit_at_current_rate(&investor1, &id, &300);

    // Advance to interval 2 (rate = 100 + 100 = 200)
    advance_ledgers(&ctx.env, 20);

    // Investor2 commits at ledger 20 → rate = 200
    ctx.client.commit_at_current_rate(&investor2, &id, &300);

    let commitments = ctx.client.get_commitments(&id);
    assert_eq!(commitments.len(), 2);

    let c1 = commitments.get(0).unwrap();
    assert_eq!(c1.investor, investor1);
    assert_eq!(c1.locked_rate_bps, 100);
    assert_eq!(c1.amount, 300);

    let c2 = commitments.get(1).unwrap();
    assert_eq!(c2.investor, investor2);
    assert_eq!(c2.locked_rate_bps, 200);
    assert_eq!(c2.amount, 300);
}

// ── Auction fills ──────────────────────────────────────────────────────────

#[test]
fn test_auction_closes_immediately_when_fully_funded() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 30);

    token.mint(&investor, &1000);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);

    ctx.client.commit_at_current_rate(&investor, &id, &1000);

    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.status, AuctionStatus::Filled);
    assert_eq!(view.auction.total_committed, 1000);
}

#[test]
fn test_auction_caps_commitment_to_remaining_capacity() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 31);

    // face_value = 500, investor tries to commit 800
    token.mint(&investor, &800);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &500, &100, &500, &50, &10);

    ctx.client.commit_at_current_rate(&investor, &id, &800);

    // Should have been capped to 500 and the auction closed as Filled.
    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.status, AuctionStatus::Filled);
    assert_eq!(view.auction.total_committed, 500);

    // Investor should have 300 tokens back (800 - 500).
    let balance = TokenClient::new(&ctx.env, &token_addr).balance(&investor);
    assert_eq!(balance, 300);
}

#[test]
fn test_funds_transferred_to_issuer_on_fill() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 32);

    token.mint(&investor, &1000);
    let tc = TokenClient::new(&ctx.env, &token_addr);
    let initial_issuer_bal = tc.balance(&issuer);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);
    ctx.client.commit_at_current_rate(&investor, &id, &1000);

    let final_issuer_bal = tc.balance(&issuer);
    assert_eq!(final_issuer_bal - initial_issuer_bal, 1000);
}

#[test]
fn test_commit_after_filled_reverts() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor = Address::generate(&ctx.env);
    let latebird = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 33);

    token.mint(&investor, &1000);
    token.mint(&latebird, &100);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);
    ctx.client.commit_at_current_rate(&investor, &id, &1000);

    // Auction is Filled; latebird commit should revert.
    let result = ctx.client.try_commit_at_current_rate(&latebird, &id, &100);
    assert!(result.is_err());
}

// ── Expiry & refunds ───────────────────────────────────────────────────────

#[test]
fn test_close_expired_refunds_all_commitments() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor1 = Address::generate(&ctx.env);
    let investor2 = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 40);

    let tc = TokenClient::new(&ctx.env, &token_addr);

    token.mint(&investor1, &300);
    token.mint(&investor2, &200);

    // face=1000 so it won't fill. step=100 per 1 ledger, start=400, max=500
    // → reaches max after 1 ledger.
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &400, &500, &100, &1);

    ctx.client.commit_at_current_rate(&investor1, &id, &300);
    ctx.client.commit_at_current_rate(&investor2, &id, &200);

    // Advance to max rate.
    advance_ledgers(&ctx.env, 1);

    let bal1_before = tc.balance(&investor1);
    let bal2_before = tc.balance(&investor2);

    ctx.client.close_expired(&id);

    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.status, AuctionStatus::Expired);

    // Both investors must receive their full amounts back.
    assert_eq!(tc.balance(&investor1) - bal1_before, 300);
    assert_eq!(tc.balance(&investor2) - bal2_before, 200);
}

#[test]
fn test_close_expired_before_max_rate_reverts() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 41);

    // max rate requires 4 intervals of 10 ledgers each; we advance only 2.
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &100, &10);

    advance_ledgers(&ctx.env, 20); // 2 intervals → rate=300, still below max=500

    let result = ctx.client.try_close_expired(&id);
    assert!(result.is_err());
}

#[test]
fn test_new_commit_after_max_rate_unfilled_reverts() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 42);

    token.mint(&investor, &100);

    // reach max in 1 ledger
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &400, &500, &100, &1);

    advance_ledgers(&ctx.env, 1); // now at max rate and still unfilled

    let result = ctx.client.try_commit_at_current_rate(&investor, &id, &100);
    assert!(result.is_err());
}

// ── Cancellation ───────────────────────────────────────────────────────────

#[test]
fn test_cancel_auction_with_no_commitments_succeeds() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 50);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);
    ctx.client.cancel_auction(&issuer, &id);

    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.status, AuctionStatus::Cancelled);
}

#[test]
fn test_cancel_after_commitment_reverts() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let investor = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 51);

    token.mint(&investor, &100);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);
    ctx.client.commit_at_current_rate(&investor, &id, &100);

    let result = ctx.client.try_cancel_auction(&issuer, &id);
    assert!(result.is_err());
}

#[test]
fn test_cancel_by_non_issuer_reverts() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let impostor = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 52);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);
    let result = ctx.client.try_cancel_auction(&impostor, &id);
    assert!(result.is_err());
}

// ── Multi-investor partial fill then expire ────────────────────────────────

#[test]
fn test_partial_commitments_then_expire_refunds_all() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let tc = TokenClient::new(&ctx.env, &token_addr);
    let id = fresh_id(&ctx.env, 60);

    // Three investors commit, total < face_value → auction expires.
    let investors: std::vec::Vec<Address> = (0..3).map(|_| Address::generate(&ctx.env)).collect();
    for inv in &investors {
        token.mint(inv, &100);
    }

    // face=1000; investors commit 300 total → unfilled at max rate.
    // step=200 per 1 ledger, start=100, max=500 → max after 2 ledgers.
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &200, &1);

    for inv in &investors {
        ctx.client.commit_at_current_rate(inv, &id, &100);
    }

    let bals_before: std::vec::Vec<i128> = investors.iter().map(|i| tc.balance(i)).collect();

    // Advance to max rate.
    advance_ledgers(&ctx.env, 2);

    ctx.client.close_expired(&id);

    for (i, inv) in investors.iter().enumerate() {
        let refunded = tc.balance(inv) - bals_before[i];
        assert_eq!(refunded, 100, "investor {} should be refunded 100", i);
    }

    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.status, AuctionStatus::Expired);
}

// ── Incremental fill ───────────────────────────────────────────────────────

#[test]
fn test_multiple_investors_fill_auction_incrementally() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, token) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 70);

    // face=600; three investors each commit 200 → filled on 3rd commit.
    let investors: std::vec::Vec<Address> = (0..3).map(|_| Address::generate(&ctx.env)).collect();
    for inv in &investors {
        token.mint(inv, &200);
    }

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &600, &100, &500, &50, &10);

    // Commit in separate ledger intervals to verify different locked rates.
    ctx.client.commit_at_current_rate(&investors[0], &id, &200);

    advance_ledgers(&ctx.env, 10); // rate → 150
    ctx.client.commit_at_current_rate(&investors[1], &id, &200);

    advance_ledgers(&ctx.env, 10); // rate → 200
    ctx.client.commit_at_current_rate(&investors[2], &id, &200);

    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.status, AuctionStatus::Filled);
    assert_eq!(view.auction.total_committed, 600);

    let commitments = view.commitments;
    assert_eq!(commitments.get(0).unwrap().locked_rate_bps, 100);
    assert_eq!(commitments.get(1).unwrap().locked_rate_bps, 150);
    assert_eq!(commitments.get(2).unwrap().locked_rate_bps, 200);
}

// ── Edge: start_rate == max_rate ───────────────────────────────────────────

#[test]
fn test_start_equals_max_rate_is_valid() {
    // start_rate == max_rate is a fixed-rate auction: valid configuration.
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 80);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &300, &300, &10, &5);

    // Rate should always be 300.
    advance_ledgers(&ctx.env, 100);
    assert_eq!(ctx.client.current_rate_bps(&id), 300);
}

// ── get_auction view ───────────────────────────────────────────────────────

#[test]
fn test_get_auction_returns_full_state() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 90);

    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &100, &500, &50, &10);

    advance_ledgers(&ctx.env, 10);

    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.face_value, 1000);
    assert_eq!(view.auction.start_rate_bps, 100);
    assert_eq!(view.auction.max_rate_bps, 500);
    assert_eq!(view.auction.step_bps, 50);
    assert_eq!(view.auction.step_ledgers, 10);
    assert_eq!(view.current_rate_bps, 150); // 100 + 1*50
    assert_eq!(view.auction.status, AuctionStatus::Open);
    assert_eq!(view.commitments.len(), 0);
}

// ── Close expired: no commitments ─────────────────────────────────────────

#[test]
fn test_close_expired_with_no_commitments_succeeds() {
    let ctx = setup();
    let issuer = Address::generate(&ctx.env);
    let (token_addr, _) = create_token(&ctx.env);
    let id = fresh_id(&ctx.env, 91);

    // Reach max in 1 ledger.
    ctx.client
        .start_rate_auction(&issuer, &id, &token_addr, &1000, &400, &500, &100, &1);

    advance_ledgers(&ctx.env, 1);

    // No commitments; close_expired should still succeed.
    ctx.client.close_expired(&id);

    let view = ctx.client.get_auction(&id);
    assert_eq!(view.auction.status, AuctionStatus::Expired);
}
