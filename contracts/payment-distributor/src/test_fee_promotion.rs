//! Fee promotion / fee tier precedence matrix (#826).
//!
//! Precedence: active promotion (`start_ledger <= ledger < end_ledger`), then
//! the first matching tier in stored order, then the caller's default fee.

use super::*;
use crate::errors::Error;
use crate::types::{FeePromotion, FeeTier};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient as AssetClient};
use soroban_sdk::{vec, Address, Env, Symbol};

const DEFAULT_BPS: i128 = 500;
const PAID: i128 = 10_000;

struct Ctx<'a> {
    env: Env,
    admin: Address,
    distributor_id: Address,
    distributor: PaymentDistributorClient<'a>,
    token: TokenClient<'a>,
    asset: AssetClient<'a>,
}

fn setup(env: &Env) -> Ctx<'_> {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let distributor_id = env.register(PaymentDistributor, ());
    let distributor = PaymentDistributorClient::new(env, &distributor_id);
    distributor.initialize(&admin);
    let token_id = env.register_stellar_asset_contract_v2(Address::generate(env));
    Ctx {
        env: env.clone(),
        admin,
        distributor_id,
        distributor,
        token: TokenClient::new(env, &token_id.address()),
        asset: AssetClient::new(env, &token_id.address()),
    }
}

fn set_ledger(env: &Env, seq: u32) {
    env.ledger().set_sequence_number(seq);
}

fn promo(fee_bps: u32, start_ledger: u32, end_ledger: u32) -> Option<FeePromotion> {
    Some(FeePromotion {
        fee_bps,
        start_ledger,
        end_ledger,
    })
}

/// Platform fee the dry-run preview reports for a fresh invoice.
fn preview_fee(ctx: &Ctx, invoice: &str) -> i128 {
    let env = &ctx.env;
    ctx.distributor
        .calculate_distribution_splits(
            &Address::generate(env),
            &Symbol::new(env, invoice),
            &vec![
                env,
                ctx.token.address.clone(),
                Address::generate(env),
                Address::generate(env),
                ctx.admin.clone(),
            ],
            &vec![env, PAID, PAID, 0i128, DEFAULT_BPS],
        )
        .platform_fee
}

/// Platform fee actually transferred to the fee recipient (admin) by execution.
fn executed_fee(ctx: &Ctx, invoice: &str) -> i128 {
    let env = &ctx.env;
    ctx.asset.mint(&ctx.distributor_id, &(PAID * 2));
    let before = ctx.token.balance(&ctx.admin);
    ctx.distributor.distribute_payment(
        &Address::generate(env),
        &Symbol::new(env, invoice),
        &vec![
            env,
            ctx.token.address.clone(),
            Address::generate(env),
            Address::generate(env),
            ctx.admin.clone(),
        ],
        &vec![env, PAID, PAID, 0i128, DEFAULT_BPS],
        &2u32,
    );
    ctx.token.balance(&ctx.admin) - before
}

#[test]
fn no_promotion_and_no_tiers_uses_default_fee() {
    let env = Env::default();
    let ctx = setup(&env);
    assert_eq!(preview_fee(&ctx, "NONE"), 500);
    assert_eq!(executed_fee(&ctx, "NONE"), 500);
}

#[test]
fn promotion_applies_only_between_start_and_expiry_ledgers() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.distributor
        .set_fee_promotion(&ctx.admin, &promo(100, 100, 200));

    // (ledger, expected fee): before start, exact start, last active, exact expiry.
    let matrix = [(99u32, 500i128), (100, 100), (199, 100), (200, 500)];
    for (i, (seq, fee)) in matrix.iter().enumerate() {
        set_ledger(&env, *seq);
        let inv = ["P0", "P1", "P2", "P3"][i];
        assert_eq!(preview_fee(&ctx, inv), *fee, "preview at ledger {}", seq);
        assert_eq!(executed_fee(&ctx, inv), *fee, "execution at ledger {}", seq);
    }
}

#[test]
fn active_promotion_overrides_tiers_and_tiers_resume_after_expiry() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.distributor.set_fee_tiers(
        &ctx.admin,
        &vec![
            &env,
            FeeTier {
                min_amount: 0,
                max_amount: 0,
                fee_bps: 300,
            },
        ],
    );
    ctx.distributor
        .set_fee_promotion(&ctx.admin, &promo(50, 10, 20));

    set_ledger(&env, 10);
    assert_eq!(preview_fee(&ctx, "T0"), 50);
    assert_eq!(executed_fee(&ctx, "T0"), 50);

    set_ledger(&env, 20);
    assert_eq!(preview_fee(&ctx, "T1"), 300);
    assert_eq!(executed_fee(&ctx, "T1"), 300);
}

#[test]
fn overlapping_tiers_resolve_to_first_stored_tier() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.distributor.set_fee_tiers(
        &ctx.admin,
        &vec![
            &env,
            FeeTier {
                min_amount: 0,
                max_amount: PAID,
                fee_bps: 200,
            },
            FeeTier {
                min_amount: PAID,
                max_amount: 0,
                fee_bps: 100,
            },
        ],
    );
    // PAID is inside both ranges; the first stored tier wins every time.
    assert_eq!(preview_fee(&ctx, "O0"), 200);
    assert_eq!(executed_fee(&ctx, "O0"), 200);
    assert_eq!(preview_fee(&ctx, "O1"), 200);
}

#[test]
fn cleared_promotion_no_longer_applies() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.distributor
        .set_fee_promotion(&ctx.admin, &promo(100, 0, 1_000));
    assert_eq!(preview_fee(&ctx, "C0"), 100);
    ctx.distributor.set_fee_promotion(&ctx.admin, &None);
    assert_eq!(ctx.distributor.get_fee_promotion(), None);
    assert_eq!(preview_fee(&ctx, "C1"), 500);
}

#[test]
fn set_fee_promotion_rejects_invalid_input_and_non_admin() {
    let env = Env::default();
    let ctx = setup(&env);
    assert_eq!(
        ctx.distributor
            .try_set_fee_promotion(&ctx.admin, &promo(100, 50, 50)),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        ctx.distributor
            .try_set_fee_promotion(&ctx.admin, &promo(10_001, 0, 50)),
        Err(Ok(Error::InvalidBps))
    );
    assert_eq!(
        ctx.distributor
            .try_set_fee_promotion(&Address::generate(&env), &promo(100, 0, 50)),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(ctx.distributor.get_fee_promotion(), None);
}
