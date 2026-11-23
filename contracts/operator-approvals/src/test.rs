use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, BytesN, Env};

struct Ctx<'a> {
    client: OperatorApprovalsClient<'a>,
    token: TokenClient<'a>,
    investor: Address,
    operator: Address,
    inv: BytesN<32>,
}

fn setup(env: &Env) -> Ctx<'_> {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token = token_id.address();
    StellarAssetClient::new(env, &token).mint(&admin, &1_000);

    let contract_id = env.register(OperatorApprovals, ());
    let client = OperatorApprovalsClient::new(env, &contract_id);
    client.initialize(&admin, &token);

    let investor = Address::generate(env);
    let inv = BytesN::from_array(env, &[1; 32]);
    client.credit_payout(&admin, &investor, &inv, &100);
    Ctx {
        client,
        token: TokenClient::new(env, &token),
        investor,
        operator: Address::generate(env),
        inv,
    }
}

#[test]
fn test_approved_operator_claim_pays_investor() {
    let env = Env::default();
    let c = setup(&env);
    c.client.approve_operator(&c.investor, &c.operator, &100);
    assert!(c.client.is_operator(&c.investor, &c.operator));

    let paid = c.client.claim_for(&c.operator, &c.investor, &c.inv);
    assert_eq!(paid, 100);
    assert_eq!(c.token.balance(&c.investor), 100);
    assert_eq!(c.token.balance(&c.operator), 0);
    assert_eq!(c.client.claimable(&c.investor, &c.inv), 0);
}

#[test]
fn test_operator_cannot_redirect_funds_to_itself() {
    let env = Env::default();
    let c = setup(&env);
    c.client.approve_operator(&c.investor, &c.operator, &100);

    let (op, inv) = (&c.operator, &c.inv);
    let res = c.client.try_claim_for(op, op, inv);
    assert_eq!(res, Err(Ok(Error::NotOperator)));
    let res = c.client.try_claim(op, inv);
    assert_eq!(res, Err(Ok(Error::NothingToClaim)));
    assert_eq!(c.token.balance(op), 0);
    assert_eq!(c.client.claimable(&c.investor, inv), 100);
}

#[test]
fn test_expired_operator_reverts() {
    let env = Env::default();
    let c = setup(&env);
    c.client.approve_operator(&c.investor, &c.operator, &100);
    env.ledger().with_mut(|li| li.sequence_number = 100);

    assert!(!c.client.is_operator(&c.investor, &c.operator));
    let res = c.client.try_claim_for(&c.operator, &c.investor, &c.inv);
    assert_eq!(res, Err(Ok(Error::NotOperator)));
}

#[test]
fn test_revoked_operator_reverts() {
    let env = Env::default();
    let c = setup(&env);
    c.client.approve_operator(&c.investor, &c.operator, &100);
    c.client.revoke_operator(&c.investor, &c.operator);

    assert!(!c.client.is_operator(&c.investor, &c.operator));
    let res = c.client.try_claim_for(&c.operator, &c.investor, &c.inv);
    assert_eq!(res, Err(Ok(Error::NotOperator)));
}

#[test]
fn test_more_than_five_operators_reverts() {
    let env = Env::default();
    let c = setup(&env);
    for _ in 0..5 {
        let op = Address::generate(&env);
        c.client.approve_operator(&c.investor, &op, &100);
    }
    let (investor, op) = (&c.investor, &c.operator);
    let res = c.client.try_approve_operator(investor, op, &100);
    assert_eq!(res, Err(Ok(Error::TooManyOperators)));

    env.ledger().with_mut(|li| li.sequence_number = 100);
    c.client.approve_operator(&c.investor, &c.operator, &200);
    assert!(c.client.is_operator(&c.investor, &c.operator));
}
