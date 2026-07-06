#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::TokenClient,
    Address, Env, String,
};

use crate::{Error, SubRailContract, SubRailContractClient};

const MONTH: u64 = 30 * 24 * 3_600;
const WEEK: u64 = 7 * 24 * 3_600;
const PRICE: i128 = 50_000_000; // 5 units of a 7-decimal token

struct Setup<'a> {
    env: Env,
    client: SubRailContractClient<'a>,
    token: TokenClient<'a>,
    merchant: Address,
}

fn setup() -> Setup<'static> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_700_000_000);

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token = TokenClient::new(&env, &sac.address());

    let contract_id = env.register(SubRailContract, ());
    let client = SubRailContractClient::new(&env, &contract_id);
    client.initialize(&admin);

    Setup {
        env,
        client,
        token,
        merchant,
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
fn set_plan_active_toggles() {
    let s = setup();
    let plan_id = create_default_plan(&s);

    s.client.set_plan_active(&plan_id, &false);
    assert!(!s.client.get_plan(&plan_id).active);

    s.client.set_plan_active(&plan_id, &true);
    assert!(s.client.get_plan(&plan_id).active);
}
