use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, BytesN, Env};

struct Ctx<'a> {
    client: EscapeHatchClient<'a>,
    token: TokenClient<'a>,
    admin: Address,
    a: Address,
    b: Address,
    inv: BytesN<32>,
}

fn set_ledger(env: &Env, sequence: u32) {
    env.ledger().with_mut(|li| li.sequence_number = sequence);
}

/// `a` funds 300 and `b` funds 100; 200 is then disbursed to the issuer,
/// leaving 200 in the vault. The protocol is paused at ledger 10.
fn setup(env: &Env) -> Ctx<'_> {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token = token_id.address();
    let asset = StellarAssetClient::new(env, &token);
    let (a, b) = (Address::generate(env), Address::generate(env));
    asset.mint(&a, &300);
    asset.mint(&b, &100);

    let contract_id = env.register(EscapeHatch, ());
    let client = EscapeHatchClient::new(env, &contract_id);
    client.initialize(&admin, &token);
    let inv = BytesN::from_array(env, &[1; 32]);
    client.list_invoice(&admin, &inv);
    client.fund(&a, &inv, &300);
    client.fund(&b, &inv, &100);
    let issuer = Address::generate(env);
    client.disburse(&admin, &inv, &issuer, &200);

    set_ledger(env, 10);
    client.pause(&admin);
    Ctx {
        client,
        token: TokenClient::new(env, &token),
        admin,
        a,
        b,
        inv,
    }
}

#[test]
fn test_escape_reverts_before_delay() {
    let env = Env::default();
    let c = setup(&env);
    set_ledger(&env, 10 + ESCAPE_DELAY_LEDGERS - 1);
    assert!(!c.client.escape_enabled());
    let res = c.client.try_escape_withdraw(&c.a, &c.inv);
    assert_eq!(res, Err(Ok(Error::EscapeNotEnabled)));
}

#[test]
fn test_escape_pays_pro_rata_principal_once() {
    let env = Env::default();
    let c = setup(&env);
    set_ledger(&env, 10 + ESCAPE_DELAY_LEDGERS);
    assert!(c.client.escape_enabled());

    assert_eq!(c.client.escape_withdraw(&c.a, &c.inv), 150);
    assert_eq!(c.client.escape_withdraw(&c.b, &c.inv), 50);
    assert_eq!(c.token.balance(&c.a), 150);
    assert_eq!(c.token.balance(&c.b), 50);
    assert_eq!(c.client.vault_balance(&c.inv), 0);

    let res = c.client.try_escape_withdraw(&c.a, &c.inv);
    assert_eq!(res, Err(Ok(Error::AlreadyWithdrawn)));
    let stranger = Address::generate(&env);
    let res = c.client.try_escape_withdraw(&stranger, &c.inv);
    assert_eq!(res, Err(Ok(Error::NoPosition)));
}

#[test]
fn test_unpause_before_delay_clears_timer() {
    let env = Env::default();
    let c = setup(&env);
    set_ledger(&env, 10 + ESCAPE_DELAY_LEDGERS - 1);
    c.client.unpause(&c.admin);
    assert_eq!(c.client.paused_since(), None);

    c.client.pause(&c.admin);
    set_ledger(&env, 10 + ESCAPE_DELAY_LEDGERS);
    assert!(!c.client.escape_enabled());
    let res = c.client.try_escape_withdraw(&c.a, &c.inv);
    assert_eq!(res, Err(Ok(Error::EscapeNotEnabled)));
}

#[test]
fn test_listing_and_funding_disabled_while_paused() {
    let env = Env::default();
    let c = setup(&env);
    set_ledger(&env, 10 + ESCAPE_DELAY_LEDGERS);
    let other = BytesN::from_array(&env, &[2; 32]);
    let res = c.client.try_list_invoice(&c.admin, &other);
    assert_eq!(res, Err(Ok(Error::Paused)));
    let res = c.client.try_fund(&c.a, &c.inv, &1);
    assert_eq!(res, Err(Ok(Error::Paused)));
}
