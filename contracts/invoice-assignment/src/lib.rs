//! Invoice assignment (novation) contract (issue #850).
//!
//! Moves the obligation to pay an invoice from one debtor to another. The
//! issuer proposes the assignment (which counts as the issuer's consent), the
//! old and new debtors each consent, and anyone can then finalize it. An
//! assignment is blocked while the invoice is disputed or once settlement has
//! started, i.e. once the first payment has been recorded.

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol,
};
use soroban_sdk::{IntoVal, Val};

#[cfg(test)]
mod test;

const TTL_THRESHOLD: u32 = 120_960;
const TTL_EXTEND: u32 = 1_036_800;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvoiceExists = 4,
    InvoiceNotFound = 5,
    InvoiceDisputed = 6,
    SettlementStarted = 7,
    AssignmentPending = 8,
    NoPendingAssignment = 9,
    MissingConsent = 10,
    SameDebtor = 11,
    NotCurrentDebtor = 12,
    InvalidAmount = 13,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invoice {
    pub issuer: Address,
    pub debtor: Address,
    pub disputed: bool,
    pub paid: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assignment {
    pub new_debtor: Address,
    pub old_consented: bool,
    pub new_consented: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Invoice(BytesN<32>),
    Pending(BytesN<32>),
}

#[contract]
pub struct InvoiceAssignment;

#[contractimpl]
impl InvoiceAssignment {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        admin.require_auth();
        let store = env.storage().instance();
        if store.has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        store.set(&DataKey::Admin, &admin);
        store.extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        Ok(())
    }

    /// Register an invoice with its issuer and current debtor.
    pub fn register_invoice(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
        debtor: Address,
    ) -> Result<(), Error> {
        issuer.require_auth();
        admin_of(&env)?;
        let key = DataKey::Invoice(invoice_id.clone());
        if env.storage().persistent().has(&key) {
            return Err(Error::InvoiceExists);
        }
        let invoice = Invoice {
            issuer,
            debtor,
            disputed: false,
            paid: 0,
        };
        set_invoice(&env, &invoice_id, &invoice);
        Ok(())
    }

    /// Admin-only: open or close a dispute on an invoice.
    pub fn set_disputed(
        env: Env,
        admin: Address,
        invoice_id: BytesN<32>,
        disputed: bool,
    ) -> Result<(), Error> {
        admin.require_auth();
        if admin_of(&env)? != admin {
            return Err(Error::Unauthorized);
        }
        let mut invoice = invoice_of(&env, &invoice_id)?;
        invoice.disputed = disputed;
        set_invoice(&env, &invoice_id, &invoice);
        Ok(())
    }

    /// Issuer proposes moving the obligation to `new_debtor`.
    pub fn propose_assignment(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
        new_debtor: Address,
    ) -> Result<(), Error> {
        issuer.require_auth();
        let invoice = invoice_of(&env, &invoice_id)?;
        if invoice.issuer != issuer {
            return Err(Error::Unauthorized);
        }
        check_assignable(&invoice)?;
        if invoice.debtor == new_debtor {
            return Err(Error::SameDebtor);
        }
        if pending_of(&env, &invoice_id).is_ok() {
            return Err(Error::AssignmentPending);
        }
        let pending = Assignment {
            new_debtor: new_debtor.clone(),
            old_consented: false,
            new_consented: false,
        };
        set_pending(&env, &invoice_id, &pending);
        let data = (invoice.debtor, new_debtor);
        emit(&env, "assignment_proposed", invoice_id, data);
        Ok(())
    }

    /// The current debtor consents to the pending assignment.
    pub fn consent_old(env: Env, debtor: Address, invoice_id: BytesN<32>) -> Result<(), Error> {
        debtor.require_auth();
        let invoice = invoice_of(&env, &invoice_id)?;
        if invoice.debtor != debtor {
            return Err(Error::Unauthorized);
        }
        let mut pending = pending_of(&env, &invoice_id)?;
        pending.old_consented = true;
        set_pending(&env, &invoice_id, &pending);
        emit(&env, "assignment_consented", invoice_id, debtor);
        Ok(())
    }

    /// The proposed new debtor consents to the pending assignment.
    pub fn consent_new(env: Env, debtor: Address, invoice_id: BytesN<32>) -> Result<(), Error> {
        debtor.require_auth();
        let mut pending = pending_of(&env, &invoice_id)?;
        if pending.new_debtor != debtor {
            return Err(Error::Unauthorized);
        }
        pending.new_consented = true;
        set_pending(&env, &invoice_id, &pending);
        emit(&env, "assignment_consented", invoice_id, debtor);
        Ok(())
    }

    /// Apply the assignment once the issuer, old and new debtor all consented.
    pub fn finalize_assignment(env: Env, invoice_id: BytesN<32>) -> Result<(), Error> {
        let mut invoice = invoice_of(&env, &invoice_id)?;
        let pending = pending_of(&env, &invoice_id)?;
        check_assignable(&invoice)?;
        if !pending.old_consented || !pending.new_consented {
            return Err(Error::MissingConsent);
        }
        let data = (invoice.debtor.clone(), pending.new_debtor.clone());
        invoice.debtor = pending.new_debtor;
        set_invoice(&env, &invoice_id, &invoice);
        let key = DataKey::Pending(invoice_id.clone());
        env.storage().persistent().remove(&key);
        emit(&env, "assignment_finalized", invoice_id, data);
        Ok(())
    }

    /// Issuer cancels a pending assignment, clearing all consents.
    pub fn cancel_assignment(
        env: Env,
        issuer: Address,
        invoice_id: BytesN<32>,
    ) -> Result<(), Error> {
        issuer.require_auth();
        let invoice = invoice_of(&env, &invoice_id)?;
        if invoice.issuer != issuer {
            return Err(Error::Unauthorized);
        }
        pending_of(&env, &invoice_id)?;
        let key = DataKey::Pending(invoice_id);
        env.storage().persistent().remove(&key);
        Ok(())
    }

    /// Record a payment. Only the current debtor can pay; the first payment
    /// starts settlement, after which the invoice can no longer be assigned.
    pub fn record_payment(
        env: Env,
        payer: Address,
        invoice_id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        payer.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut invoice = invoice_of(&env, &invoice_id)?;
        if invoice.debtor != payer {
            return Err(Error::NotCurrentDebtor);
        }
        invoice.paid += amount;
        set_invoice(&env, &invoice_id, &invoice);
        Ok(())
    }

    pub fn get_invoice(env: Env, invoice_id: BytesN<32>) -> Result<Invoice, Error> {
        invoice_of(&env, &invoice_id)
    }

    pub fn get_pending(env: Env, invoice_id: BytesN<32>) -> Option<Assignment> {
        pending_of(&env, &invoice_id).ok()
    }
}

fn emit<D: IntoVal<Env, Val>>(env: &Env, name: &str, invoice_id: BytesN<32>, data: D) {
    let topics = (Symbol::new(env, name), invoice_id);
    env.events().publish(topics, data);
}

fn admin_of(env: &Env) -> Result<Address, Error> {
    let store = env.storage().instance();
    store.extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
    store.get(&DataKey::Admin).ok_or(Error::NotInitialized)
}

fn check_assignable(invoice: &Invoice) -> Result<(), Error> {
    if invoice.disputed {
        return Err(Error::InvoiceDisputed);
    }
    if invoice.paid > 0 {
        return Err(Error::SettlementStarted);
    }
    Ok(())
}

fn invoice_of(env: &Env, invoice_id: &BytesN<32>) -> Result<Invoice, Error> {
    let key = DataKey::Invoice(invoice_id.clone());
    let store = env.storage().persistent();
    let invoice = store.get(&key).ok_or(Error::InvoiceNotFound)?;
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
    Ok(invoice)
}

fn set_invoice(env: &Env, invoice_id: &BytesN<32>, invoice: &Invoice) {
    let key = DataKey::Invoice(invoice_id.clone());
    let store = env.storage().persistent();
    store.set(&key, invoice);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}

fn pending_of(env: &Env, invoice_id: &BytesN<32>) -> Result<Assignment, Error> {
    let key = DataKey::Pending(invoice_id.clone());
    let store = env.storage().persistent();
    store.get(&key).ok_or(Error::NoPendingAssignment)
}

fn set_pending(env: &Env, invoice_id: &BytesN<32>, pending: &Assignment) {
    let key = DataKey::Pending(invoice_id.clone());
    let store = env.storage().persistent();
    store.set(&key, pending);
    store.extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}
