#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, Env, String,
};

use crate::{ChargeOutcome, Error, SubRailContract, SubRailContractClient, SubscriptionStatus};

const MONTH: u64 = 30 * 24 * 3_600;
const WEEK: u64 = 7 * 24 * 3_600;
const PRICE: i128 = 50_000_000; // 5 units of a 7-decimal token

struct Setup<'a> {
    env: Env,
    client: SubRailContractClient<'a>,
    token: TokenClient<'a>,
    merchant: Address,
    subscriber: Address,
}

fn setup() -> Setup<'static> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_700_000_000);

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token = TokenClient::new(&env, &sac.address());
    StellarAssetClient::new(&env, &sac.address()).mint(&subscriber, &10_000_000_000);

    let contract_id = env.register(SubRailContract, ());
    let client = SubRailContractClient::new(&env, &contract_id);
    client.initialize(&admin);

    Setup {
        env,
        client,
        token,
        merchant,
        subscriber,
    }
}

fn create_default_plan(s: &Setup) -> u64 {
    s.client.create_plan(
        &s.merchant,
        &s.token.address,
        &PRICE,
        &MONTH,
        &WEEK,
        &String::from_str(&s.env, "Pro Monthly"),
    )
}

fn advance(env: &Env, secs: u64) {
    env.ledger().with_mut(|l| l.timestamp += secs);
}

// ── Setup & plans ──────────────────────────────────────────────────────────

#[test]
fn initialize_only_once() {
    let s = setup();
    let again = Address::generate(&s.env);
    assert_eq!(
        s.client.try_initialize(&again),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn get_version_returns_1() {
    let s = setup();
    assert_eq!(s.client.get_version(), 1);
}

#[test]
fn create_plan_stores_and_indexes() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let plan = s.client.get_plan(&plan_id);

    assert_eq!(plan.id, plan_id);
    assert_eq!(plan.merchant, s.merchant);
    assert_eq!(plan.amount, PRICE);
    assert_eq!(plan.interval, MONTH);
    assert!(plan.active);
    assert_eq!(s.client.get_merchant_plans(&s.merchant).len(), 1);
}

#[test]
fn create_plan_rejects_bad_inputs() {
    let s = setup();
    let name = String::from_str(&s.env, "x");
    assert_eq!(
        s.client
            .try_create_plan(&s.merchant, &s.token.address, &0, &MONTH, &WEEK, &name),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        s.client
            .try_create_plan(&s.merchant, &s.token.address, &PRICE, &60, &WEEK, &name),
        Err(Ok(Error::InvalidInterval))
    );
}

#[test]
fn set_plan_active_toggles_and_blocks_new_subs() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    s.client.set_plan_active(&plan_id, &false);
    assert!(!s.client.get_plan(&plan_id).active);

    assert_eq!(
        s.client
            .try_subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 3)),
        Err(Ok(Error::PlanInactive))
    );
}

// ── Subscribe ──────────────────────────────────────────────────────────────

#[test]
fn subscribe_charges_first_period_immediately() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 3));

    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.periods_paid, 1);
    assert_eq!(sub.total_spent, PRICE);
    assert_eq!(sub.balance, PRICE * 2);
    assert_eq!(s.token.balance(&s.merchant), PRICE);
    assert_eq!(sub.next_charge_at, 1_700_000_000 + MONTH);
}

#[test]
fn subscribe_rejects_cap_below_plan_price() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    assert_eq!(
        s.client
            .try_subscribe(&s.subscriber, &plan_id, &(PRICE - 1), &0, &(PRICE * 3)),
        Err(Ok(Error::CapBelowPlanAmount))
    );
}

#[test]
fn subscribe_rejects_deposit_below_first_charge() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    assert_eq!(
        s.client
            .try_subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE - 1)),
        Err(Ok(Error::DepositBelowFirstCharge))
    );
}

// ── Charge / keeper flow ───────────────────────────────────────────────────

#[test]
fn charge_settles_when_due() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 3));

    advance(&s.env, MONTH);
    assert!(s.client.is_charge_due(&sub_id));
    assert_eq!(s.client.charge(&sub_id), ChargeOutcome::Charged);

    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.periods_paid, 2);
    assert_eq!(sub.balance, PRICE);
    assert_eq!(s.token.balance(&s.merchant), PRICE * 2);
}

#[test]
fn charge_too_early_is_rejected() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 3));

    advance(&s.env, MONTH - 10);
    assert!(!s.client.is_charge_due(&sub_id));
    assert_eq!(s.client.try_charge(&sub_id), Err(Ok(Error::ChargeNotDue)));
}

#[test]
fn underfunded_charge_moves_to_past_due_then_recovers() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    // Deposit only covers the first period.
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &PRICE);

    advance(&s.env, MONTH);
    assert_eq!(s.client.charge(&sub_id), ChargeOutcome::InsufficientFunds);
    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::PastDue);
    assert_eq!(sub.failed_attempts, 1);

    // Top up within the grace window and retry.
    s.client.deposit(&sub_id, &(PRICE * 2));
    let recovered_at = s.env.ledger().timestamp() + 3_600;
    advance(&s.env, 3_600);
    assert_eq!(s.client.charge(&sub_id), ChargeOutcome::Charged);

    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.failed_attempts, 0);
    assert_eq!(sub.periods_paid, 2);
    // Recovery re-anchors the schedule at the recovery time.
    assert_eq!(sub.next_charge_at, recovered_at + MONTH);
}

#[test]
fn grace_elapsed_expires_and_refunds_residual() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    // Leave a residual smaller than one period: deposit 1.5x price.
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE + PRICE / 2));
    let wallet_after_subscribe = s.token.balance(&s.subscriber);

    // Past due date AND past the grace window.
    advance(&s.env, MONTH + WEEK + 1);
    assert_eq!(s.client.charge(&sub_id), ChargeOutcome::GraceElapsed);

    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Expired);
    assert_eq!(sub.balance, 0);
    // Residual half-period came back to the subscriber's wallet.
    assert_eq!(
        s.token.balance(&s.subscriber),
        wallet_after_subscribe + PRICE / 2
    );
    // Terminal: further charges are invalid.
    assert_eq!(s.client.try_charge(&sub_id), Err(Ok(Error::InvalidStatus)));
}

#[test]
fn spend_ceiling_completes_subscription() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    // Ceiling of exactly two periods.
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &(PRICE * 2), &(PRICE * 5));

    advance(&s.env, MONTH);
    assert_eq!(s.client.charge(&sub_id), ChargeOutcome::Charged);

    advance(&s.env, MONTH);
    assert_eq!(s.client.charge(&sub_id), ChargeOutcome::CeilingReached);
    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Expired);
    assert_eq!(sub.total_spent, PRICE * 2);
}

// ── Deposit / withdraw ─────────────────────────────────────────────────────

#[test]
fn withdraw_keeps_subscriber_in_control() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 3));

    // Pull everything unused back out.
    s.client.withdraw(&sub_id, &(PRICE * 2));
    assert_eq!(s.client.get_balance(&sub_id), 0);

    // Over-withdrawal is impossible.
    assert_eq!(
        s.client.try_withdraw(&sub_id, &1),
        Err(Ok(Error::InsufficientBalance))
    );
}

// ── Pause / resume / cancel ────────────────────────────────────────────────

#[test]
fn pause_shifts_schedule_by_paused_duration() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 3));
    let original_due = s.client.get_subscription(&sub_id).next_charge_at;

    advance(&s.env, WEEK);
    s.client.pause(&sub_id);
    assert_eq!(
        s.client.get_subscription(&sub_id).status,
        SubscriptionStatus::Paused
    );

    // Charging while paused is invalid even after the original due date.
    advance(&s.env, MONTH);
    assert_eq!(s.client.try_charge(&sub_id), Err(Ok(Error::InvalidStatus)));

    s.client.resume(&sub_id);
    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    // Schedule shifted by exactly the month spent paused.
    assert_eq!(sub.next_charge_at, original_due + MONTH);
}

#[test]
fn cancel_refunds_full_balance_and_is_terminal() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let sub_id = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 3));
    let wallet_before = s.token.balance(&s.subscriber);

    s.client.cancel(&sub_id);

    let sub = s.client.get_subscription(&sub_id);
    assert_eq!(sub.status, SubscriptionStatus::Cancelled);
    assert_eq!(sub.balance, 0);
    assert_eq!(s.token.balance(&s.subscriber), wallet_before + PRICE * 2);

    // Terminal: no deposit, no double cancel, no charge.
    assert_eq!(
        s.client.try_deposit(&sub_id, &PRICE),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(s.client.try_cancel(&sub_id), Err(Ok(Error::InvalidStatus)));
    assert_eq!(s.client.try_charge(&sub_id), Err(Ok(Error::InvalidStatus)));
}

// ── Indexes ────────────────────────────────────────────────────────────────

#[test]
fn subscriber_index_tracks_all_subscriptions() {
    let s = setup();
    let plan_id = create_default_plan(&s);
    let a = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 2));
    let b = s
        .client
        .subscribe(&s.subscriber, &plan_id, &PRICE, &0, &(PRICE * 2));

    let ids = s.client.get_subscriber_subscriptions(&s.subscriber);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids.get(0), Some(a));
    assert_eq!(ids.get(1), Some(b));
}
