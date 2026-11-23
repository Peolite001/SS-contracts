#![cfg(test)]

use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, token, Address, BytesN, Env};

use crate::{Error, ExitFeeContract, ExitFeeContractClient};

struct Setup {
    env: Env,
    admin: Address,
    issuer: Address,
    investor: Address,
    reserve: Address,
    inv: BytesN<32>,
    token: token::Client<'static>,
    client: ExitFeeContractClient<'static>,
}

/// Fee falls from 500 bps to 100 bps over ledgers 900..=1_000; the invoice
/// deadline is ledger 1_000 with a 5_000 minimum, and the investor has
/// committed 6_000 so the minimum is met.
fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let token_id = sac.address();
    let investor = Address::generate(&env);
    let minter = token::StellarAssetClient::new(&env, &token_id);
    minter.mint(&investor, &10_000);
    let token = token::Client::new(&env, &token_id);

    let id = env.register(ExitFeeContract, ());
    let client = ExitFeeContractClient::new(&env, &id);
    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let reserve = Address::generate(&env);
    client.initialize(&admin, &token_id, &reserve, &1_000);
    client.set_exit_fee_schedule(&admin, &500, &100, &100);
    let inv = BytesN::from_array(&env, &[1; 32]);
    client.open_invoice(&issuer, &inv, &1_000, &5_000);
    client.commit(&investor, &inv, &6_000);
    Setup {
        env,
        admin,
        issuer,
        investor,
        reserve,
        inv,
        token,
        client,
    }
}

fn set_ledger(env: &Env, sequence: u32) {
    env.ledger().set_sequence_number(sequence);
}

#[test]
fn fee_equals_start_and_end_at_window_bounds() {
    let s = setup();
    set_ledger(&s.env, 899);
    assert_eq!(s.client.exit_fee_bps(&s.inv), 500);
    set_ledger(&s.env, 900);
    assert_eq!(s.client.exit_fee_bps(&s.inv), 500);
    set_ledger(&s.env, 950);
    assert_eq!(s.client.exit_fee_bps(&s.inv), 300);
    set_ledger(&s.env, 999);
    assert_eq!(s.client.exit_fee_bps(&s.inv), 104);
    set_ledger(&s.env, 1_000);
    assert_eq!(s.client.exit_fee_bps(&s.inv), 100);
}

#[test]
fn refund_plus_fee_equals_commitment_and_matches_preview() {
    let s = setup();
    set_ledger(&s.env, 950);
    let (refund, fee) = s.client.preview_exit(&s.inv, &s.investor);
    let paid = s.client.withdraw_commitment(&s.investor, &s.inv);
    assert_eq!(paid, refund);
    assert_eq!(refund + fee, 6_000);
    assert_eq!(fee, 180);
    assert_eq!(s.token.balance(&s.reserve), 180);
    assert_eq!(s.token.balance(&s.investor), 9_820);
}

#[test]
fn failed_funding_waives_fee() {
    let s = setup();
    let inv = BytesN::from_array(&s.env, &[2; 32]);
    s.client.open_invoice(&s.issuer, &inv, &1_000, &50_000);
    s.client.commit(&s.investor, &inv, &1_000);
    set_ledger(&s.env, 1_000);
    assert_eq!(s.client.exit_fee_bps(&inv), 0);
    assert_eq!(s.client.preview_exit(&inv, &s.investor), (1_000, 0));
    assert_eq!(s.client.withdraw_commitment(&s.investor, &inv), 1_000);
    assert_eq!(s.token.balance(&s.reserve), 0);
}

#[test]
fn fee_above_max_reverts_at_configuration() {
    let s = setup();
    let c = &s.client;
    let res = c.try_set_exit_fee_schedule(&s.admin, &1_001, &1, &1);
    assert_eq!(res, Err(Ok(Error::FeeTooHigh)));
    let res = c.try_set_exit_fee_schedule(&s.admin, &1, &1_001, &1);
    assert_eq!(res, Err(Ok(Error::FeeTooHigh)));
}
