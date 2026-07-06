use soroban_sdk::{contracttype, Address, Env, Vec};

use crate::errors::Error;
use crate::types::{Plan, Subscription};

/// ~30 days of ledgers (5s close time) — refreshed on every touch.
const PERSISTENT_TTL_THRESHOLD: u32 = 259_200;
const PERSISTENT_TTL_EXTEND_TO: u32 = 518_400;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    PlanCounter,
    SubCounter,
    Plan(u64),
    Subscription(u64),
    MerchantPlans(Address),
    SubscriberSubs(Address),
}

pub fn has_admin(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Admin)
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn get_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotInitialized)
}

pub fn next_plan_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PlanCounter)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&DataKey::PlanCounter, &id);
    id
}

pub fn next_sub_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::SubCounter)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&DataKey::SubCounter, &id);
    id
}

pub fn set_plan(env: &Env, plan: &Plan) {
    let key = DataKey::Plan(plan.id);
    env.storage().persistent().set(&key, plan);
    extend_ttl(env, &key);
}

pub fn get_plan(env: &Env, plan_id: u64) -> Result<Plan, Error> {
    let key = DataKey::Plan(plan_id);
    let plan = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::PlanNotFound)?;
    extend_ttl(env, &key);
    Ok(plan)
}

pub fn set_subscription(env: &Env, sub: &Subscription) {
    let key = DataKey::Subscription(sub.id);
    env.storage().persistent().set(&key, sub);
    extend_ttl(env, &key);
}

pub fn get_subscription(env: &Env, sub_id: u64) -> Result<Subscription, Error> {
    let key = DataKey::Subscription(sub_id);
    let sub = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::SubscriptionNotFound)?;
    extend_ttl(env, &key);
    Ok(sub)
}

pub fn push_merchant_plan(env: &Env, merchant: &Address, plan_id: u64) {
    let key = DataKey::MerchantPlans(merchant.clone());
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    ids.push_back(plan_id);
    env.storage().persistent().set(&key, &ids);
    extend_ttl(env, &key);
}

pub fn get_merchant_plans(env: &Env, merchant: &Address) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::MerchantPlans(merchant.clone()))
        .unwrap_or_else(|| Vec::new(env))
}

pub fn push_subscriber_sub(env: &Env, subscriber: &Address, sub_id: u64) {
    let key = DataKey::SubscriberSubs(subscriber.clone());
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    ids.push_back(sub_id);
    env.storage().persistent().set(&key, &ids);
    extend_ttl(env, &key);
}

pub fn get_subscriber_subs(env: &Env, subscriber: &Address) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::SubscriberSubs(subscriber.clone()))
        .unwrap_or_else(|| Vec::new(env))
}

fn extend_ttl(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_EXTEND_TO);
}
