//! Invoice storage TTL refresh and retention regression tests (#773).

use super::*;
use crate::storage::{MIN_TTL_EXTEND, TTL_THRESHOLD};
use crate::types::StorageKey;
use soroban_sdk::testutils::storage::Persistent as _;
use soroban_sdk::testutils::{Address as _, Ledger as _};

/// Absolute ledger until which `key` stays live.
fn live_until(env: &Env, id: &Address, key: &StorageKey) -> u32 {
    env.as_contract(id, || {
        env.ledger().sequence() + env.storage().persistent().get_ttl(key)
    })
}

#[test]
fn invoice_ttl_refresh_never_shortens_retention_and_preserves_data() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(InvoiceEscrow, ());
    let client = InvoiceEscrowClient::new(&env, &id);
    client.initialize(&Address::generate(&env), &300);

    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let invoice_id = BytesN::from_array(&env, &[77u8; 32]);
    client.create_invoice(
        &invoice_id,
        &Address::generate(&env),
        &1_000_000,
        &100_000,
        &1_000_000,
        &1_000,
        &0,
        &None,
        &token,
    );
    let original = client.get_invoice(&invoice_id);
    let key = StorageKey::Invoice(invoice_id.clone());
    let created = live_until(&env, &id, &key);
    assert!(created >= MIN_TTL_EXTEND);

    // Above the refresh threshold an extension is a no-op, never a shortening.
    env.ledger().set_sequence_number(1_000);
    client.batch_extend_ttl(&soroban_sdk::vec![&env, key.clone()]);
    assert_eq!(live_until(&env, &id, &key), created);

    // Below the threshold the entry is pushed out to a full extension again.
    let near_expiry = created - TTL_THRESHOLD + 1;
    env.ledger().set_sequence_number(near_expiry);
    client.batch_extend_ttl(&soroban_sdk::vec![&env, key.clone()]);
    assert!(live_until(&env, &id, &key) >= near_expiry + MIN_TTL_EXTEND);
    assert_eq!(client.get_invoice(&invoice_id), original);

    // Unknown keys are skipped rather than treated as live.
    let missing = StorageKey::Invoice(BytesN::from_array(&env, &[78u8; 32]));
    client.batch_extend_ttl(&soroban_sdk::vec![&env, missing.clone()]);
    assert!(!env.as_contract(&id, || env.storage().persistent().has(&missing)));
}
