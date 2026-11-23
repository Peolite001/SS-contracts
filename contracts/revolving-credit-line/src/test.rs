#![cfg(test)]

use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, BytesN, Env};

use crate::{Error, RevolvingCreditLine, RevolvingCreditLineClient};

struct Setup {
    env: Env,
    admin: Address,
    issuer: Address,
    client: RevolvingCreditLineClient<'static>,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(RevolvingCreditLine, ());
    let client = RevolvingCreditLineClient::new(&env, &id);
    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    client.initialize(&admin);
    client.open_line(&admin, &issuer, &1_000, &0);
    Setup {
        env,
        admin,
        issuer,
        client,
    }
}

fn invoice(env: &Env, seed: u8) -> BytesN<32> {
    BytesN::from_array(env, &[seed; 32])
}

#[test]
fn draw_above_available_reverts() {
    let s = setup();
    let inv = invoice(&s.env, 1);
    s.client.draw(&s.issuer, &inv, &600);
    let res = s.client.try_draw(&s.issuer, &inv, &401);
    assert_eq!(res, Err(Ok(Error::LimitExceeded)));
    assert_eq!(s.client.available(&s.issuer), 400);
}

#[test]
fn repay_frees_capacity_equal_to_principal() {
    let s = setup();
    let inv = invoice(&s.env, 1);
    s.client.draw(&s.issuer, &inv, &700);
    assert_eq!(s.client.available(&s.issuer), 300);
    s.client.repay(&s.issuer, &inv, &250);
    assert_eq!(s.client.available(&s.issuer), 550);
    assert_eq!(s.client.get_line(&s.issuer).drawn, 450);
}

#[test]
fn interest_accrues_per_ledger() {
    let s = setup();
    let issuer = Address::generate(&s.env);
    s.client.open_line(&s.admin, &issuer, &1_000_000, &10);
    let inv = invoice(&s.env, 2);
    s.client.draw(&issuer, &inv, &100_000);
    s.env.ledger().with_mut(|li| li.sequence_number += 50);
    // 100_000 drawn * 10 bps * 50 ledgers = 5_000 interest
    assert_eq!(s.client.available(&issuer), 895_000);
    s.client.repay(&issuer, &inv, &5_000);
    let line = s.client.get_line(&issuer);
    assert_eq!(line.accrued_interest, 0);
    assert_eq!(line.drawn, 100_000);
}

#[test]
fn frozen_line_rejects_draws_but_accepts_repayment() {
    let s = setup();
    let inv = invoice(&s.env, 3);
    s.client.draw(&s.issuer, &inv, &400);
    s.client.freeze_line(&s.admin, &s.issuer);
    let res = s.client.try_draw(&s.issuer, &inv, &1);
    assert_eq!(res, Err(Ok(Error::LineFrozen)));
    s.client.repay(&s.issuer, &inv, &400);
    assert_eq!(s.client.get_line(&s.issuer).drawn, 0);
}

#[test]
fn close_with_outstanding_balance_reverts() {
    let s = setup();
    let inv = invoice(&s.env, 4);
    s.client.draw(&s.issuer, &inv, &10);
    let res = s.client.try_close_line(&s.admin, &s.issuer);
    assert_eq!(res, Err(Ok(Error::OutstandingBalance)));
    s.client.repay(&s.issuer, &inv, &10);
    s.client.close_line(&s.admin, &s.issuer);
    let res = s.client.try_get_line(&s.issuer);
    assert_eq!(res, Err(Ok(Error::LineNotFound)));
}
