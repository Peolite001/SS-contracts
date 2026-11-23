#![allow(deprecated)]

use soroban_sdk::{BytesN, Env, Symbol, Vec};

use crate::types::{ClassAllocation, PayeeClass};

/// Emitted when the contract is initialized.
pub fn initialized(env: &Env, admin: &soroban_sdk::Address) {
    let topics = (Symbol::new(env, "initialized"),);
    env.events().publish(topics, admin.clone());
}

/// Emitted when the admin updates the priority order.
/// Data: the new ordered Vec<u32> of PayeeClass discriminants.
pub fn priority_set(env: &Env, admin: &soroban_sdk::Address, order: &Vec<PayeeClass>) {
    let topics = (Symbol::new(env, "priority_set"), admin.clone());
    // Publish discriminants as u32 so off-chain listeners need no ABI.
    let mut discriminants = Vec::new(env);
    for class in order.iter() {
        discriminants.push_back(class as u32);
    }
    env.events().publish(topics, discriminants);
}

/// Emitted once per PayeeClass within a `distribute_by_priority` call.
///
/// Topics: ("class_paid", invoice_id)
/// Data:   (class: u32, amount: i128, shortfall: i128)
pub fn class_paid(env: &Env, invoice_id: &BytesN<32>, allocation: &ClassAllocation) {
    let topics = (Symbol::new(env, "class_paid"), invoice_id.clone());
    env.events().publish(
        topics,
        (
            allocation.class as u32,
            allocation.amount,
            allocation.shortfall,
        ),
    );
}

/// Emitted after all classes have been processed for a distribution.
///
/// Topics: ("distribution_complete", invoice_id)
/// Data:   (total_paid: i128, timestamp: u64)
pub fn distribution_complete(env: &Env, invoice_id: &BytesN<32>, total_paid: i128) {
    let topics = (
        Symbol::new(env, "distribution_complete"),
        invoice_id.clone(),
    );
    let timestamp = env.ledger().timestamp();
    env.events().publish(topics, (total_paid, timestamp));
}
