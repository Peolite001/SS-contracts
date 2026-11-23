use soroban_sdk::{
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient},
    Address, BytesN, Env,
};

use crate::{EpochStatus, Error, Invoice, StakingRewards, StakingRewardsClient, EPOCH_DURATION};

struct Setup {
    env: Env,
    first: Address,
    second: Address,
    token: TokenClient<'static>,
    invoice_id: BytesN<32>,
    client: StakingRewardsClient<'static>,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    let issuer = Address::generate(&env);
    let first = Address::generate(&env);
    let second = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let mint = StellarAssetClient::new(&env, &token_id);
    mint.mint(&issuer, &10_000);
    mint.mint(&first, &1_000);
    mint.mint(&second, &1_000);
    let token = TokenClient::new(&env, &token_id);

    let contract_id = env.register(StakingRewards, ());
    let client = StakingRewardsClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let invoice_id = BytesN::from_array(&env, &[7; 32]);
    client.register_invoice(
        &invoice_id,
        &Invoice {
            issuer: issuer.clone(),
            token: token_id,
            bonus_bps: 0,
        },
    );

    Setup {
        env,
        first,
        second,
        token,
        invoice_id,
        client,
    }
}

fn set_timestamp(env: &Env, timestamp: u64) {
    env.ledger().with_mut(|ledger| ledger.timestamp = timestamp);
}

#[test]
fn epoch_snapshots_funds_and_rolls_unclaimed_rewards_forward() {
    let s = setup();
    s.client
        .stake_fractions(&s.first, &s.invoice_id, &100, &(EPOCH_DURATION * 3));
    s.client
        .stake_fractions(&s.second, &s.invoice_id, &300, &(EPOCH_DURATION * 3));
    s.client.fund_staking_pool(&s.invoice_id, &400);
    set_timestamp(&s.env, 100);

    let first_epoch = s.client.start_epoch(&s.invoice_id);
    assert_eq!(first_epoch.id, 0);
    assert_eq!(first_epoch.started_at, 100);
    assert_eq!(first_epoch.ends_at, 100 + EPOCH_DURATION);
    assert_eq!(first_epoch.stake_snapshot, 400);
    assert_eq!(first_epoch.reward_pool, 400);
    assert_eq!(s.client.claimable_amount(&0, &0), 100);
    assert_eq!(s.client.claimable_amount(&1, &0), 300);

    s.client.fund_staking_pool(&s.invoice_id, &100);
    assert_eq!(s.client.claimable_amount(&0, &0), 100);
    assert_eq!(s.client.claim_rewards(&s.first, &0, &0), 100);

    set_timestamp(&s.env, first_epoch.ends_at);
    assert_eq!(s.client.close_epoch(&s.invoice_id), 300);
    assert_eq!(
        s.client.epoch_status(&s.invoice_id, &0).status,
        EpochStatus::Closed
    );

    let second_epoch = s.client.start_epoch(&s.invoice_id);
    assert_eq!(second_epoch.id, 1);
    assert_eq!(second_epoch.reward_pool, 400);
    assert_eq!(second_epoch.stake_snapshot, 400);
}

#[test]
fn partial_claims_leave_the_unclaimed_share_and_rounding_dust_to_carry() {
    let s = setup();
    s.client
        .stake_fractions(&s.first, &s.invoice_id, &100, &EPOCH_DURATION);
    s.client
        .stake_fractions(&s.second, &s.invoice_id, &100, &EPOCH_DURATION);
    s.client.fund_staking_pool(&s.invoice_id, &101);
    let epoch = s.client.start_epoch(&s.invoice_id);

    assert_eq!(s.client.claimable_amount(&0, &0), 50);
    assert_eq!(s.client.claim_rewards(&s.first, &0, &0), 50);
    assert_eq!(s.token.balance(&s.first), 950);

    set_timestamp(&s.env, epoch.ends_at);
    assert_eq!(s.client.close_epoch(&s.invoice_id), 51);
    assert_eq!(s.client.epoch_status(&s.invoice_id, &0).claimed_amount, 50);
}

#[test]
fn closed_epoch_cannot_be_claimed_or_closed_again() {
    let s = setup();
    s.client
        .stake_fractions(&s.first, &s.invoice_id, &100, &EPOCH_DURATION);
    s.client.fund_staking_pool(&s.invoice_id, &100);
    let epoch = s.client.start_epoch(&s.invoice_id);

    set_timestamp(&s.env, epoch.ends_at);
    assert_eq!(s.client.close_epoch(&s.invoice_id), 100);
    assert_eq!(
        s.client.try_claim_rewards(&s.first, &0, &0),
        Err(Ok(Error::EpochNotOpen))
    );
    assert_eq!(
        s.client.try_close_epoch(&s.invoice_id),
        Err(Ok(Error::NoActiveEpoch))
    );
}

#[test]
fn stakes_added_after_epoch_start_are_not_eligible_for_that_epoch() {
    let s = setup();
    s.client
        .stake_fractions(&s.first, &s.invoice_id, &100, &EPOCH_DURATION);
    s.client.fund_staking_pool(&s.invoice_id, &100);
    s.client.start_epoch(&s.invoice_id);
    s.client
        .stake_fractions(&s.second, &s.invoice_id, &100, &EPOCH_DURATION);

    assert_eq!(s.client.claimable_amount(&1, &0), 0);
    assert_eq!(
        s.client.try_claim_rewards(&s.second, &1, &0),
        Err(Ok(Error::NotEligible))
    );
}
