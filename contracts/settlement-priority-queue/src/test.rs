//! Unit tests for the settlement-priority-queue contract.
//!
//! Coverage:
//!   - Initialization and re-initialization guard
//!   - set_priority validation (duplicates, missing classes, wrong length)
//!   - distribute_by_priority: full proceeds, shortfall, reordered priority
//!   - preview_distribution matches distribute_by_priority
//!   - Sum of payments always equals amount distributed
//!   - AlreadyDistributed guard on a second distribution call

#![cfg(test)]

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    vec, Address, BytesN, Env,
};

use crate::{
    DistributionResult, InvoiceOwed, PayeeClass, SettlementPriorityQueue,
    SettlementPriorityQueueClient,
};

// ── Helpers ────────────────────────────────────────────────────────────────

fn setup() -> (Env, Address, SettlementPriorityQueueClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SettlementPriorityQueue, ());
    let client = SettlementPriorityQueueClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, admin, client)
}

/// Default owed: 100 each for the five classes (total 500).
fn owed_equal(env: &Env) -> InvoiceOwed {
    let _ = env;
    InvoiceOwed {
        platform_fee: 100,
        insurance_pool: 100,
        senior_investors: 100,
        junior_investors: 100,
        issuer_residual: 100,
    }
}

/// Standard priority order: PlatformFee → InsurancePool → Senior → Junior → Issuer.
fn standard_order(env: &Env) -> soroban_sdk::Vec<PayeeClass> {
    vec![
        env,
        PayeeClass::PlatformFee,
        PayeeClass::InsurancePool,
        PayeeClass::SeniorInvestors,
        PayeeClass::JuniorInvestors,
        PayeeClass::IssuerResidual,
    ]
}

fn fresh_invoice_id(env: &Env, seed: u8) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    BytesN::from_array(env, &bytes)
}

// ── Initialization ─────────────────────────────────────────────────────────

#[test]
fn test_initialize_sets_admin() {
    let (env, admin, client) = setup();
    let stored = client.get_admin();
    assert_eq!(stored, admin);
    let _ = env;
}

#[test]
fn test_initialize_rejects_reinit() {
    let (env, admin, client) = setup();
    let result = client.try_initialize(&admin);
    assert!(result.is_err());
    let _ = env;
}

// ── set_priority validation ────────────────────────────────────────────────

#[test]
fn test_set_priority_accepts_valid_order() {
    let (env, admin, client) = setup();
    let order = standard_order(&env);
    client.set_priority(&admin, &order);
    let stored = client.get_priority().unwrap();
    assert_eq!(stored, order);
}

#[test]
fn test_set_priority_rejects_duplicates() {
    let (env, admin, client) = setup();
    // Duplicate: PlatformFee appears twice, IssuerResidual missing.
    let bad = vec![
        &env,
        PayeeClass::PlatformFee,
        PayeeClass::PlatformFee,
        PayeeClass::InsurancePool,
        PayeeClass::SeniorInvestors,
        PayeeClass::JuniorInvestors,
    ];
    let result = client.try_set_priority(&admin, &bad);
    assert!(result.is_err());
}

#[test]
fn test_set_priority_rejects_wrong_length() {
    let (env, admin, client) = setup();
    // Only four entries.
    let bad = vec![
        &env,
        PayeeClass::PlatformFee,
        PayeeClass::InsurancePool,
        PayeeClass::SeniorInvestors,
        PayeeClass::JuniorInvestors,
    ];
    let result = client.try_set_priority(&admin, &bad);
    assert!(result.is_err());
}

#[test]
fn test_set_priority_rejects_missing_class() {
    let (env, admin, client) = setup();
    // Five entries but IssuerResidual replaced by a duplicate InsurancePool.
    let bad = vec![
        &env,
        PayeeClass::PlatformFee,
        PayeeClass::InsurancePool,
        PayeeClass::InsurancePool,
        PayeeClass::SeniorInvestors,
        PayeeClass::JuniorInvestors,
    ];
    let result = client.try_set_priority(&admin, &bad);
    assert!(result.is_err());
}

#[test]
fn test_set_priority_unauthorized() {
    let (env, _admin, client) = setup();
    let impostor = Address::generate(&env);
    let order = standard_order(&env);
    let result = client.try_set_priority(&impostor, &order);
    assert!(result.is_err());
}

// ── Full proceeds ──────────────────────────────────────────────────────────

#[test]
fn test_full_proceeds_pays_every_class_completely() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 1);
    let owed = owed_equal(&env);
    client.register_invoice(&admin, &id, &owed);

    // 500 = sum of all owed amounts.
    let result = client.distribute_by_priority(&id, &500);

    assert_eq!(result.total_paid, 500);
    for alloc in result.allocations.iter() {
        assert_eq!(
            alloc.amount, 100,
            "class {:?} should be fully paid",
            alloc.class
        );
        assert_eq!(
            alloc.shortfall, 0,
            "class {:?} should have zero shortfall",
            alloc.class
        );
    }
}

#[test]
fn test_full_proceeds_sum_equals_amount() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 2);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    let amount = 500_i128;
    let result = client.distribute_by_priority(&id, &amount);

    let sum: i128 = result.allocations.iter().map(|a| a.amount).sum();
    assert_eq!(sum, amount);
}

// ── Shortfall ──────────────────────────────────────────────────────────────

#[test]
fn test_shortfall_lower_classes_receive_zero() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 3);
    // PlatformFee=100, InsurancePool=100, Senior=100, Junior=100, Issuer=100
    client.register_invoice(&admin, &id, &owed_equal(&env));

    // Only enough to fully pay PlatformFee + InsurancePool + half of Senior.
    // order: PlatformFee(100) → InsurancePool(100) → Senior(100) → Junior(100) → Issuer(100)
    // amount = 250: PlatformFee=100(full), Insurance=100(full), Senior=50(partial), Junior=0, Issuer=0
    let result = client.distribute_by_priority(&id, &250);

    assert_eq!(result.total_paid, 250);

    let allocs = result.allocations;
    // Index 0: PlatformFee
    let platform = allocs.get(0).unwrap();
    assert_eq!(platform.class, PayeeClass::PlatformFee);
    assert_eq!(platform.amount, 100);
    assert_eq!(platform.shortfall, 0);

    // Index 1: InsurancePool
    let insurance = allocs.get(1).unwrap();
    assert_eq!(insurance.class, PayeeClass::InsurancePool);
    assert_eq!(insurance.amount, 100);
    assert_eq!(insurance.shortfall, 0);

    // Index 2: SeniorInvestors — partial payment
    let senior = allocs.get(2).unwrap();
    assert_eq!(senior.class, PayeeClass::SeniorInvestors);
    assert_eq!(senior.amount, 50);
    assert_eq!(senior.shortfall, 50);

    // Index 3: JuniorInvestors — zero (shortfall = full owed)
    let junior = allocs.get(3).unwrap();
    assert_eq!(junior.class, PayeeClass::JuniorInvestors);
    assert_eq!(junior.amount, 0);
    assert_eq!(junior.shortfall, 100);

    // Index 4: IssuerResidual — zero
    let issuer = allocs.get(4).unwrap();
    assert_eq!(issuer.class, PayeeClass::IssuerResidual);
    assert_eq!(issuer.amount, 0);
    assert_eq!(issuer.shortfall, 100);
}

#[test]
fn test_shortfall_sum_of_payments_equals_amount() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 4);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    let amount = 150_i128;
    let result = client.distribute_by_priority(&id, &amount);

    let sum: i128 = result.allocations.iter().map(|a| a.amount).sum();
    assert_eq!(sum, amount);
    assert_eq!(result.total_paid, amount);
}

#[test]
fn test_zero_amount_reverts() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 5);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    assert!(client.try_distribute_by_priority(&id, &0).is_err());
}

// ── Reordered priority ────────────────────────────────────────────────────

#[test]
fn test_reordered_priority_junior_before_senior() {
    let (env, admin, client) = setup();

    // Junior before Senior in priority (unusual but valid to configure).
    let reversed = vec![
        &env,
        PayeeClass::JuniorInvestors,
        PayeeClass::SeniorInvestors,
        PayeeClass::InsurancePool,
        PayeeClass::PlatformFee,
        PayeeClass::IssuerResidual,
    ];
    client.set_priority(&admin, &reversed);

    let id = fresh_invoice_id(&env, 6);
    // Junior=200, Senior=200, rest=0
    let owed = InvoiceOwed {
        platform_fee: 0,
        insurance_pool: 0,
        senior_investors: 200,
        junior_investors: 200,
        issuer_residual: 0,
    };
    client.register_invoice(&admin, &id, &owed);

    // Only 250 available; Junior gets 200 (full), Senior gets 50 (partial).
    let result = client.distribute_by_priority(&id, &250);

    assert_eq!(result.total_paid, 250);

    let allocs = result.allocations;
    // Priority order: Junior, Senior, Insurance, Platform, Issuer
    let junior = allocs.get(0).unwrap();
    assert_eq!(junior.class, PayeeClass::JuniorInvestors);
    assert_eq!(junior.amount, 200);
    assert_eq!(junior.shortfall, 0);

    let senior = allocs.get(1).unwrap();
    assert_eq!(senior.class, PayeeClass::SeniorInvestors);
    assert_eq!(senior.amount, 50);
    assert_eq!(senior.shortfall, 150);
}

#[test]
fn test_reordered_priority_sum_still_equals_amount() {
    let (env, admin, client) = setup();

    let order = vec![
        &env,
        PayeeClass::IssuerResidual,
        PayeeClass::JuniorInvestors,
        PayeeClass::SeniorInvestors,
        PayeeClass::InsurancePool,
        PayeeClass::PlatformFee,
    ];
    client.set_priority(&admin, &order);

    let id = fresh_invoice_id(&env, 7);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    let amount = 333_i128;
    let result = client.distribute_by_priority(&id, &amount);

    let sum: i128 = result.allocations.iter().map(|a| a.amount).sum();
    assert_eq!(sum, amount);
}

// ── preview_distribution ───────────────────────────────────────────────────

#[test]
fn test_preview_matches_actual_distribution() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 8);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    let amount = 275_i128;

    // Preview is a read-only call; run it first.
    let preview = client.preview_distribution(&id, &amount);

    // Now actually distribute.
    let actual = client.distribute_by_priority(&id, &amount);

    // The allocations must be identical.
    assert_eq!(preview.total_paid, actual.total_paid);
    assert_eq!(preview.allocations.len(), actual.allocations.len());

    for i in 0..preview.allocations.len() {
        let p = preview.allocations.get(i).unwrap();
        let a = actual.allocations.get(i).unwrap();
        assert_eq!(p.class, a.class);
        assert_eq!(p.amount, a.amount);
        assert_eq!(p.shortfall, a.shortfall);
    }
}

#[test]
fn test_preview_does_not_mutate_state() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 9);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    // Call preview twice — both should succeed identically.
    let p1 = client.preview_distribution(&id, &100);
    let p2 = client.preview_distribution(&id, &100);

    assert_eq!(p1.total_paid, p2.total_paid);
}

// ── AlreadyDistributed guard ───────────────────────────────────────────────

#[test]
fn test_already_distributed_rejects_second_call() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 10);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    // First call succeeds.
    client.distribute_by_priority(&id, &500);

    // Second call must revert.
    let result = client.try_distribute_by_priority(&id, &500);
    assert!(result.is_err());
}

// ── Edge cases ─────────────────────────────────────────────────────────────

#[test]
fn test_distribute_without_priority_set_reverts() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SettlementPriorityQueue, ());
    let client = SettlementPriorityQueueClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    // Register an invoice but do NOT call set_priority.
    let id = fresh_invoice_id(&env, 11);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    let result = client.try_distribute_by_priority(&id, &100);
    assert!(result.is_err());
}

#[test]
fn test_distribute_unregistered_invoice_reverts() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let unknown_id = fresh_invoice_id(&env, 12);
    let result = client.try_distribute_by_priority(&unknown_id, &100);
    assert!(result.is_err());
}

#[test]
fn test_register_invoice_duplicate_reverts() {
    let (env, admin, client) = setup();
    let id = fresh_invoice_id(&env, 13);
    client.register_invoice(&admin, &id, &owed_equal(&env));

    let result = client.try_register_invoice(&admin, &id, &owed_equal(&env));
    assert!(result.is_err());
}

#[test]
fn test_exact_shortfall_first_class_gets_nothing() {
    // amount = 0 would be rejected; test with amount=1 and owed where first
    // class is owed 100 (amount < owed).
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 14);
    let owed = InvoiceOwed {
        platform_fee: 100,
        insurance_pool: 100,
        senior_investors: 100,
        junior_investors: 100,
        issuer_residual: 100,
    };
    client.register_invoice(&admin, &id, &owed);

    // Only 1 token available.
    let result = client.distribute_by_priority(&id, &1);

    assert_eq!(result.total_paid, 1);

    let allocs = result.allocations;
    // PlatformFee gets 1, all others get 0.
    let platform = allocs.get(0).unwrap();
    assert_eq!(platform.amount, 1);
    assert_eq!(platform.shortfall, 99);

    for i in 1..5 {
        let a = allocs.get(i).unwrap();
        assert_eq!(a.amount, 0);
        assert_eq!(a.shortfall, 100);
    }
}

#[test]
fn test_uneven_owed_amounts_full_proceeds() {
    let (env, admin, client) = setup();
    client.set_priority(&admin, &standard_order(&env));

    let id = fresh_invoice_id(&env, 15);
    let owed = InvoiceOwed {
        platform_fee: 50,
        insurance_pool: 150,
        senior_investors: 300,
        junior_investors: 400,
        issuer_residual: 100,
    };
    client.register_invoice(&admin, &id, &owed);

    let total = 50 + 150 + 300 + 400 + 100; // 1000
    let result = client.distribute_by_priority(&id, &total);

    assert_eq!(result.total_paid, total);
    let allocs = result.allocations;

    assert_eq!(allocs.get(0).unwrap().amount, 50);
    assert_eq!(allocs.get(1).unwrap().amount, 150);
    assert_eq!(allocs.get(2).unwrap().amount, 300);
    assert_eq!(allocs.get(3).unwrap().amount, 400);
    assert_eq!(allocs.get(4).unwrap().amount, 100);

    for i in 0..5 {
        assert_eq!(allocs.get(i).unwrap().shortfall, 0);
    }
}
