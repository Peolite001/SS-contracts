use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env};

struct Ctx<'a> {
    client: InvoiceAssignmentClient<'a>,
    admin: Address,
    issuer: Address,
    old: Address,
    new: Address,
    id: BytesN<32>,
}

fn setup(env: &Env) -> Ctx<'_> {
    env.mock_all_auths();
    let contract_id = env.register(InvoiceAssignment, ());
    let client = InvoiceAssignmentClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    let issuer = Address::generate(env);
    let old = Address::generate(env);
    let new = Address::generate(env);
    let id = BytesN::from_array(env, &[1; 32]);
    client.register_invoice(&issuer, &id, &old);
    Ctx {
        client,
        admin,
        issuer,
        old,
        new,
        id,
    }
}

#[test]
fn test_full_assignment_flow_moves_payment_rights() {
    let env = Env::default();
    let c = setup(&env);
    c.client.propose_assignment(&c.issuer, &c.id, &c.new);
    c.client.consent_old(&c.old, &c.id);
    c.client.consent_new(&c.new, &c.id);
    c.client.finalize_assignment(&c.id);

    assert_eq!(c.client.get_invoice(&c.id).debtor, c.new);
    assert!(c.client.get_pending(&c.id).is_none());

    let res = c.client.try_record_payment(&c.old, &c.id, &10);
    assert_eq!(res, Err(Ok(Error::NotCurrentDebtor)));
    c.client.record_payment(&c.new, &c.id, &10);
    assert_eq!(c.client.get_invoice(&c.id).paid, 10);
}

#[test]
fn test_finalize_requires_all_consents() {
    let env = Env::default();
    let c = setup(&env);
    let res = c.client.try_finalize_assignment(&c.id);
    assert_eq!(res, Err(Ok(Error::NoPendingAssignment)));

    c.client.propose_assignment(&c.issuer, &c.id, &c.new);
    let res = c.client.try_finalize_assignment(&c.id);
    assert_eq!(res, Err(Ok(Error::MissingConsent)));

    c.client.consent_old(&c.old, &c.id);
    let res = c.client.try_finalize_assignment(&c.id);
    assert_eq!(res, Err(Ok(Error::MissingConsent)));
    assert_eq!(c.client.get_invoice(&c.id).debtor, c.old);
}

#[test]
fn test_consent_must_come_from_correct_address() {
    let env = Env::default();
    let c = setup(&env);
    let stranger = Address::generate(&env);
    c.client.propose_assignment(&c.issuer, &c.id, &c.new);

    let res = c.client.try_consent_old(&stranger, &c.id);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
    let res = c.client.try_consent_old(&c.new, &c.id);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
    let res = c.client.try_consent_new(&stranger, &c.id);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
    let res = c.client.try_consent_new(&c.old, &c.id);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));

    let res = c.client.try_propose_assignment(&stranger, &c.id, &c.new);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_cancellation_clears_pending_consents() {
    let env = Env::default();
    let c = setup(&env);
    c.client.propose_assignment(&c.issuer, &c.id, &c.new);
    c.client.consent_old(&c.old, &c.id);
    c.client.consent_new(&c.new, &c.id);
    c.client.cancel_assignment(&c.issuer, &c.id);

    assert!(c.client.get_pending(&c.id).is_none());
    let res = c.client.try_finalize_assignment(&c.id);
    assert_eq!(res, Err(Ok(Error::NoPendingAssignment)));

    c.client.propose_assignment(&c.issuer, &c.id, &c.new);
    let res = c.client.try_finalize_assignment(&c.id);
    assert_eq!(res, Err(Ok(Error::MissingConsent)));
}

#[test]
fn test_assignment_blocked_for_disputed_or_settling_invoice() {
    let env = Env::default();
    let c = setup(&env);
    c.client.set_disputed(&c.admin, &c.id, &true);
    let res = c.client.try_propose_assignment(&c.issuer, &c.id, &c.new);
    assert_eq!(res, Err(Ok(Error::InvoiceDisputed)));

    c.client.set_disputed(&c.admin, &c.id, &false);
    c.client.propose_assignment(&c.issuer, &c.id, &c.new);
    c.client.consent_old(&c.old, &c.id);
    c.client.consent_new(&c.new, &c.id);
    c.client.record_payment(&c.old, &c.id, &5);

    let res = c.client.try_finalize_assignment(&c.id);
    assert_eq!(res, Err(Ok(Error::SettlementStarted)));
    assert_eq!(c.client.get_invoice(&c.id).debtor, c.old);
}
