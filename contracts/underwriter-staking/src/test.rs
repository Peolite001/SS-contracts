#![cfg(test)]

use soroban_sdk::{testutils::Address as _, token, Address, BytesN, Env};

use crate::{Error, UnderwriterStaking, UnderwriterStakingClient};

struct Setup {
    env: Env,
    admin: Address,
    issuer: Address,
    u1: Address,
    u2: Address,
    inv: BytesN<32>,
    token: token::Client<'static>,
    client: UnderwriterStakingClient<'static>,
}

/// Face value 10_000 with a 20% cover cap, so at most 2_000 can be staked.
/// Every party starts with 10_000 tokens.
fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let token_id = sac.address();
    let minter = token::StellarAssetClient::new(&env, &token_id);
    let issuer = Address::generate(&env);
    let u1 = Address::generate(&env);
    let u2 = Address::generate(&env);
    minter.mint(&issuer, &10_000);
    minter.mint(&u1, &10_000);
    minter.mint(&u2, &10_000);
    let token = token::Client::new(&env, &token_id);

    let id = env.register(UnderwriterStaking, ());
    let client = UnderwriterStakingClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin, &token_id, &2_000);
    let inv = BytesN::from_array(&env, &[1; 32]);
    client.register_invoice(&issuer, &inv, &10_000);
    Setup {
        env,
        admin,
        issuer,
        u1,
        u2,
        inv,
        token,
        client,
    }
}

#[test]
fn underwriting_above_cap_reverts() {
    let s = setup();
    s.client.underwrite(&s.u1, &s.inv, &1_500, &200);
    let res = s.client.try_underwrite(&s.u2, &s.inv, &501, &200);
    assert_eq!(res, Err(Ok(Error::CoverExceeded)));
    assert_eq!(s.client.cover_of(&s.inv), (1_500, 500));
}

#[test]
fn partial_loss_is_absorbed_pro_rata_before_investors() {
    let s = setup();
    s.client.underwrite(&s.u1, &s.inv, &1_500, &200);
    s.client.underwrite(&s.u2, &s.inv, &500, &200);
    let investor_loss = s.client.apply_default_loss(&s.admin, &s.inv, &1_000);
    assert_eq!(investor_loss, 0);
    assert_eq!(s.token.balance(&s.admin), 1_000);
    assert_eq!(s.client.release_stake(&s.u1, &s.inv), 750);
    assert_eq!(s.client.release_stake(&s.u2, &s.inv), 250);
}

#[test]
fn loss_beyond_total_stake_falls_to_investors() {
    let s = setup();
    s.client.underwrite(&s.u1, &s.inv, &1_500, &200);
    s.client.underwrite(&s.u2, &s.inv, &500, &200);
    let investor_loss = s.client.apply_default_loss(&s.admin, &s.inv, &5_000);
    assert_eq!(investor_loss, 3_000);
    assert_eq!(s.token.balance(&s.admin), 2_000);
    assert_eq!(s.client.release_stake(&s.u1, &s.inv), 0);
    assert_eq!(s.client.cover_of(&s.inv), (0, 2_000));
}

#[test]
fn settled_invoice_returns_full_stake_and_keeps_premium() {
    let s = setup();
    s.client.underwrite(&s.u1, &s.inv, &1_000, &300);
    s.client.pay_premiums(&s.issuer, &s.inv);
    assert_eq!(s.token.balance(&s.u1), 9_030);
    s.client.settle(&s.admin, &s.inv);
    assert_eq!(s.client.release_stake(&s.u1, &s.inv), 1_000);
    assert_eq!(s.token.balance(&s.u1), 10_030);
    assert_eq!(s.token.balance(&s.issuer), 9_970);
}

#[test]
fn stake_cannot_be_released_while_active() {
    let s = setup();
    s.client.underwrite(&s.u1, &s.inv, &1_000, &300);
    let res = s.client.try_release_stake(&s.u1, &s.inv);
    assert_eq!(res, Err(Ok(Error::StillActive)));
    let other = Address::generate(&s.env);
    let res = s.client.try_release_stake(&other, &s.inv);
    assert_eq!(res, Err(Ok(Error::StillActive)));
}
