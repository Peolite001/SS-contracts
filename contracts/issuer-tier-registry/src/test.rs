#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env};

use crate::{Error, IssuerTierRegistry, IssuerTierRegistryClient, Outcome, Tier};

struct Setup {
    env: Env,
    escrow: Address,
    issuer: Address,
    client: IssuerTierRegistryClient<'static>,
}

/// Silver at 2 settlements, Gold at 4, 2 clean settlements to recover.
fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(IssuerTierRegistry, ());
    let client = IssuerTierRegistryClient::new(&env, &id);
    let admin = Address::generate(&env);
    let escrow = Address::generate(&env);
    let issuer = Address::generate(&env);
    client.initialize(&admin, &escrow);
    client.set_thresholds(&admin, &2, &4, &2);
    client.set_limits(&admin, &Tier::Bronze, &1_000, &2_000);
    client.set_limits(&admin, &Tier::Silver, &5_000, &10_000);
    client.set_limits(&admin, &Tier::Gold, &20_000, &50_000);
    Setup {
        env,
        escrow,
        issuer,
        client,
    }
}

fn record(s: &Setup, seed: u8, outcome: Outcome) -> Tier {
    let id = BytesN::from_array(&s.env, &[seed; 32]);
    let (escrow, issuer) = (&s.escrow, &s.issuer);
    s.client.record_outcome(escrow, issuer, &id, &outcome)
}

fn register(s: &Setup, seed: u8, value: i128) -> Option<Error> {
    let id = BytesN::from_array(&s.env, &[seed; 32]);
    let (escrow, issuer) = (&s.escrow, &s.issuer);
    let c = &s.client;
    match c.try_register_invoice(escrow, issuer, &id, &value) {
        Ok(_) => None,
        Err(Ok(e)) => Some(e),
        Err(Err(_)) => panic!("unexpected invoke error"),
    }
}

#[test]
fn new_issuer_starts_at_bronze_with_lowest_limits() {
    let s = setup();
    assert_eq!(s.client.tier_of(&s.issuer), Tier::Bronze);
    let limits = s.client.limits_for(&Tier::Bronze);
    assert_eq!(limits.max_invoice_value, 1_000);
    assert_eq!(limits.max_open_exposure, 2_000);
}

#[test]
fn settlement_threshold_promotes_one_tier() {
    let s = setup();
    assert_eq!(record(&s, 1, Outcome::Settled), Tier::Bronze);
    assert_eq!(record(&s, 2, Outcome::Settled), Tier::Silver);
    assert_eq!(record(&s, 3, Outcome::Settled), Tier::Silver);
    assert_eq!(record(&s, 4, Outcome::Settled), Tier::Gold);
}

#[test]
fn default_drops_one_tier_and_needs_clean_settlements_to_recover() {
    let s = setup();
    record(&s, 1, Outcome::Settled);
    assert_eq!(record(&s, 2, Outcome::Settled), Tier::Silver);
    assert_eq!(record(&s, 3, Outcome::Defaulted), Tier::Bronze);
    assert_eq!(record(&s, 4, Outcome::Settled), Tier::Bronze);
    assert_eq!(record(&s, 5, Outcome::Settled), Tier::Silver);
    assert_eq!(s.client.history(&s.issuer).default_count, 1);
}

#[test]
fn invoices_above_tier_limits_revert_at_creation() {
    let s = setup();
    assert_eq!(register(&s, 1, 1_001), Some(Error::TierLimitExceeded));
    assert_eq!(register(&s, 2, 1_000), None);
    assert_eq!(register(&s, 3, 1_000), None);
    // open exposure is now 2_000, the Bronze maximum
    assert_eq!(register(&s, 4, 1), Some(Error::TierLimitExceeded));
    record(&s, 2, Outcome::Settled);
    assert_eq!(register(&s, 4, 1), None);
}

#[test]
fn only_escrow_can_record_outcomes() {
    let s = setup();
    let other = Address::generate(&s.env);
    let id = BytesN::from_array(&s.env, &[1; 32]);
    let outcome = Outcome::Settled;
    let c = &s.client;
    let res = c.try_record_outcome(&other, &s.issuer, &id, &outcome);
    assert_eq!(res, Err(Ok(Error::Unauthorized)));
}
