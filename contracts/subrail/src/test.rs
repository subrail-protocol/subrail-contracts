#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, Env, String,
};

use crate::{Error, SubRailContract, SubRailContractClient, SubscriptionStatus};

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
