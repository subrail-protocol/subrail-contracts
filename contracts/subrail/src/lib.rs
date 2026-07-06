//! # SubRail — recurring payments protocol for Soroban
//!
//! Contract core. Domain modules: [`types`], [`errors`], [`events`],
//! [`storage`]. Merchant, subscriber, and keeper entry points land in
//! later milestones; this registers the contract with its admin
//! bootstrap and version query.
#![no_std]

pub mod errors;
pub mod events;
pub mod storage;
pub mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, Address, Env, String, Vec};

pub use crate::errors::Error;
pub use crate::types::{ChargeOutcome, Plan, Subscription, SubscriptionStatus};

// ── Validation constants ────────────────────────────────────────────────

/// Contract-level version, surfaced by `get_version()`.
const CONTRACT_VERSION: u32 = 1;
/// Lower bound for a billing interval (1 hour) — guards against
/// accidental per-second billing draining a balance instantly.
const MIN_INTERVAL_SECS: u64 = 3_600;
/// Upper bound for a billing interval (366 days).
const MAX_INTERVAL_SECS: u64 = 31_622_400;

#[contract]
pub struct SubRailContract;

#[contractimpl]
impl SubRailContract {
    // ── Setup ───────────────────────────────────────────────────────────────

    /// Initialize the contract with an admin address. Callable once.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if storage::has_admin(&env) {
            return Err(Error::AlreadyInitialized);
        }
        storage::set_admin(&env, &admin);
        Ok(())
    }

    // ── Merchant actions ────────────────────────────────────────────────────

    /// Create a billing plan. Returns the new plan id.
    pub fn create_plan(
        env: Env,
        merchant: Address,
        token: Address,
        amount: i128,
        interval: u64,
        grace_period: u64,
        name: String,
    ) -> Result<u64, Error> {
        merchant.require_auth();
        storage::get_admin(&env)?;

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if !(MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&interval) {
            return Err(Error::InvalidInterval);
        }

        let id = storage::next_plan_id(&env);
        let plan = Plan {
            id,
            merchant: merchant.clone(),
            token,
            amount,
            interval,
            grace_period,
            name,
            active: true,
            created_at: env.ledger().timestamp(),
        };
        storage::set_plan(&env, &plan);
        storage::push_merchant_plan(&env, &merchant, id);
        events::plan_created(&env, id, &merchant, amount, interval);
        Ok(id)
    }

    /// Open or close a plan to new subscriptions. Existing subscriptions
    /// keep billing; deactivation only stops new sign-ups.
    pub fn set_plan_active(env: Env, plan_id: u64, active: bool) -> Result<(), Error> {
        let mut plan = storage::get_plan(&env, plan_id)?;
        plan.merchant.require_auth();
        if plan.active != active {
            plan.active = active;
            storage::set_plan(&env, &plan);
            events::plan_status_changed(&env, plan_id, active);
        }
        Ok(())
    }

    // ── Read-only queries ───────────────────────────────────────────────────

    pub fn get_plan(env: Env, plan_id: u64) -> Result<Plan, Error> {
        storage::get_plan(&env, plan_id)
    }

    pub fn get_merchant_plans(env: Env, merchant: Address) -> Vec<u64> {
        storage::get_merchant_plans(&env, &merchant)
    }

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        storage::get_admin(&env)
    }

    /// Contract-level version constant.
    pub fn get_version(_env: Env) -> u32 {
        CONTRACT_VERSION
    }
}
