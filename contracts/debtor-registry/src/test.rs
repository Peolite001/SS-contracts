use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env};

struct Ctx<'a> {
    client: DebtorRegistryClient<'a>,
    admin: Address,
    issuer: Address,
    debtor: Address,
    hash: BytesN<32>,
    inv: BytesN<32>,
}

fn setup(env: &Env) -> Ctx<'_> {
    env.mock_all_auths();
    let contract_id = env.register(DebtorRegistry, ());
    let client = DebtorRegistryClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    Ctx {
        client,
        admin,
        issuer: Address::generate(env),
        debtor: Address::generate(env),
        hash: BytesN::from_array(env, &[7; 32]),
        inv: BytesN::from_array(env, &[1; 32]),
    }
}

fn set_status(env: &Env, c: &Ctx, status: DebtorStatus, seed: u8) {
    let reason = BytesN::from_array(env, &[seed; 32]);
    let (admin, hash) = (&c.admin, &c.hash);
    c.client.set_debtor_status(admin, hash, &status, &reason);
}

fn create(c: &Ctx) -> Option<Error> {
    let (issuer, debtor) = (&c.issuer, &c.debtor);
    let cl = &c.client;
    let res = cl.try_create_invoice(issuer, &c.inv, &c.hash, debtor);
    res.err().map(|e| e.unwrap())
}

fn fundable(c: &Ctx) -> Option<Error> {
    let res = c.client.try_check_fundable(&c.inv);
    res.err().map(|e| e.unwrap())
}

#[test]
fn test_unknown_debtor_is_neutral_and_fundable() {
    let env = Env::default();
    let c = setup(&env);
    assert_eq!(c.client.status_of(&c.hash), DebtorStatus::Neutral);
    assert_eq!(create(&c), None);
    assert_eq!(fundable(&c), None);
}

#[test]
fn test_blocked_debtor_reverts_at_creation_and_funding() {
    let env = Env::default();
    let c = setup(&env);
    assert_eq!(create(&c), None);

    set_status(&env, &c, DebtorStatus::Blocked, 1);
    assert_eq!(fundable(&c), Some(Error::DebtorBlocked));

    let c2 = Ctx {
        inv: BytesN::from_array(&env, &[2; 32]),
        ..c
    };
    assert_eq!(create(&c2), Some(Error::DebtorBlocked));
}

#[test]
fn test_watch_debtor_requires_acknowledgement() {
    let env = Env::default();
    let c = setup(&env);
    set_status(&env, &c, DebtorStatus::Watch, 1);
    assert_eq!(create(&c), None);
    assert_eq!(fundable(&c), Some(Error::AcknowledgementRequired));

    c.client.acknowledge(&c.inv);
    assert_eq!(fundable(&c), None);
}

#[test]
fn test_trusted_debtor_skips_acknowledgement() {
    let env = Env::default();
    let c = setup(&env);
    set_status(&env, &c, DebtorStatus::Trusted, 1);
    assert_eq!(create(&c), None);
    assert_eq!(fundable(&c), None);
}

#[test]
fn test_only_admin_can_change_status() {
    let env = Env::default();
    let c = setup(&env);
    let other = Address::generate(&env);
    let reason = BytesN::from_array(&env, &[1; 32]);
    let status = DebtorStatus::Blocked;
    let cl = &c.client;
    let res = cl.try_set_debtor_status(&other, &c.hash, &status, &reason);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
    assert_eq!(c.client.status_of(&c.hash), DebtorStatus::Neutral);
}

#[test]
fn test_history_keeps_latest_ten_changes() {
    let env = Env::default();
    let c = setup(&env);
    for seed in 0..12u8 {
        set_status(&env, &c, DebtorStatus::Watch, seed);
    }
    let history = c.client.history_of(&c.hash);
    assert_eq!(history.len(), 10);
    let first = history.get(0).unwrap();
    let last = history.get(9).unwrap();
    assert_eq!(first.reason_hash, BytesN::from_array(&env, &[2; 32]));
    assert_eq!(last.reason_hash, BytesN::from_array(&env, &[11; 32]));
    assert_eq!(last.status, DebtorStatus::Watch);
}
