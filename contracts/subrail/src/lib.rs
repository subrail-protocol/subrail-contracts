//! # SubRail — recurring payments protocol for Soroban
//!
//! SubRail lets merchants define billing plans and subscribers authorize
//! capped, non-custodial recurring pull-payments against them.
//!
//! ## Model
//!
//! - A **Plan** is immutable after creation (except its `active` flag), so
//!   what a subscriber agreed to can never silently change.
//! - A **Subscription** carries a subscriber-set per-period cap
//!   (`max_amount`) and an optional lifetime `spend_ceiling`.
//! - The subscriber prepays into a per-subscription **balance** held by
//!   this contract, withdrawable at any time. `charge` can move at most
//!   one period's price per elapsed interval from that balance to the
//!   merchant — never more, never early.
//! - `charge` is permissionless so any keeper can settle due periods.
//!   Payment failure does not revert: it transitions the subscription to
//!   `PastDue`, and to `Expired` once the plan's grace window elapses.
#![no_std]

pub mod errors;
pub mod events;
pub mod storage;
pub mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, token, Address, Env, String, Vec};

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

    // ── Subscriber actions ──────────────────────────────────────────────────

    /// Subscribe to a plan. Transfers `initial_deposit` from the
    /// subscriber into the contract, then immediately settles the first
    /// period. Returns the new subscription id.
    ///
    /// - `max_amount` is the subscriber's hard per-period cap and must be
    ///   at least the plan price.
    /// - `spend_ceiling` optionally bounds lifetime spend (`0` = none).
    pub fn subscribe(
        env: Env,
        subscriber: Address,
        plan_id: u64,
        max_amount: i128,
        spend_ceiling: i128,
        initial_deposit: i128,
    ) -> Result<u64, Error> {
        subscriber.require_auth();

        let plan = storage::get_plan(&env, plan_id)?;
        if !plan.active {
            return Err(Error::PlanInactive);
        }
        if max_amount < plan.amount {
            return Err(Error::CapBelowPlanAmount);
        }
        if spend_ceiling < 0 || initial_deposit <= 0 {
            return Err(Error::InvalidAmount);
        }
        if initial_deposit < plan.amount {
            return Err(Error::DepositBelowFirstCharge);
        }
        if spend_ceiling > 0 && spend_ceiling < plan.amount {
            return Err(Error::InvalidAmount);
        }

        // Pull the prepaid balance into the contract.
        let contract_addr = env.current_contract_address();
        token::TokenClient::new(&env, &plan.token).transfer(
            &subscriber,
            &contract_addr,
            &initial_deposit,
        );

        let now = env.ledger().timestamp();
        let id = storage::next_sub_id(&env);
        let mut sub = Subscription {
            id,
            plan_id,
            subscriber: subscriber.clone(),
            status: SubscriptionStatus::Active,
            max_amount,
            spend_ceiling,
            balance: initial_deposit,
            total_spent: 0,
            next_charge_at: now,
            periods_paid: 0,
            failed_attempts: 0,
            paused_at: 0,
            created_at: now,
        };

        // Settle the first period immediately.
        Self::settle_period(&env, &plan, &mut sub, now);

        storage::set_subscription(&env, &sub);
        storage::push_subscriber_sub(&env, &subscriber, id);
        events::subscribed(&env, id, plan_id, &subscriber, max_amount);
        Ok(id)
    }

    /// Add funds to a subscription's prepaid balance.
    pub fn deposit(env: Env, sub_id: u64, amount: i128) -> Result<(), Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut sub = storage::get_subscription(&env, sub_id)?;
        sub.subscriber.require_auth();
        if matches!(
            sub.status,
            SubscriptionStatus::Cancelled | SubscriptionStatus::Expired
        ) {
            return Err(Error::InvalidStatus);
        }

        let plan = storage::get_plan(&env, sub.plan_id)?;
        let contract_addr = env.current_contract_address();
        token::TokenClient::new(&env, &plan.token).transfer(
            &sub.subscriber,
            &contract_addr,
            &amount,
        );
        sub.balance += amount;
        storage::set_subscription(&env, &sub);
        events::deposited(&env, sub_id, amount, sub.balance);
        Ok(())
    }

    /// Withdraw unused funds from a subscription's balance at any time.
    /// The subscriber stays in control: withdrawing below one period's
    /// price simply means the next charge attempt will fail into PastDue.
    pub fn withdraw(env: Env, sub_id: u64, amount: i128) -> Result<(), Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut sub = storage::get_subscription(&env, sub_id)?;
        sub.subscriber.require_auth();
        if amount > sub.balance {
            return Err(Error::InsufficientBalance);
        }

        let plan = storage::get_plan(&env, sub.plan_id)?;
        sub.balance -= amount;
        storage::set_subscription(&env, &sub);
        token::TokenClient::new(&env, &plan.token).transfer(
            &env.current_contract_address(),
            &sub.subscriber,
            &amount,
        );
        events::withdrawn(&env, sub_id, amount, sub.balance);
        Ok(())
    }

    /// Pause billing. Only an Active subscription can be paused; the
    /// schedule is shifted by the paused duration on resume.
    pub fn pause(env: Env, sub_id: u64) -> Result<(), Error> {
        let mut sub = storage::get_subscription(&env, sub_id)?;
        sub.subscriber.require_auth();
        if sub.status != SubscriptionStatus::Active {
            return Err(Error::InvalidStatus);
        }
        let prev = sub.status;
        sub.status = SubscriptionStatus::Paused;
        sub.paused_at = env.ledger().timestamp();
        storage::set_subscription(&env, &sub);
        events::status_changed(&env, sub_id, prev, sub.status);
        Ok(())
    }

    /// Resume a paused subscription, shifting `next_charge_at` forward by
    /// exactly the time spent paused, so no period is lost or double-billed.
    pub fn resume(env: Env, sub_id: u64) -> Result<(), Error> {
        let mut sub = storage::get_subscription(&env, sub_id)?;
        sub.subscriber.require_auth();
        if sub.status != SubscriptionStatus::Paused {
            return Err(Error::InvalidStatus);
        }
        let prev = sub.status;
        let paused_for = env.ledger().timestamp().saturating_sub(sub.paused_at);
        sub.next_charge_at += paused_for;
        sub.paused_at = 0;
        sub.status = SubscriptionStatus::Active;
        storage::set_subscription(&env, &sub);
        events::status_changed(&env, sub_id, prev, sub.status);
        Ok(())
    }

    /// Cancel a subscription and refund the entire remaining balance to
    /// the subscriber. One click, no dark patterns. Terminal.
    pub fn cancel(env: Env, sub_id: u64) -> Result<(), Error> {
        let mut sub = storage::get_subscription(&env, sub_id)?;
        sub.subscriber.require_auth();
        if matches!(
            sub.status,
            SubscriptionStatus::Cancelled | SubscriptionStatus::Expired
        ) {
            return Err(Error::InvalidStatus);
        }

        let prev = sub.status;
        let refund = sub.balance;
        sub.balance = 0;
        sub.status = SubscriptionStatus::Cancelled;
        storage::set_subscription(&env, &sub);

        if refund > 0 {
            let plan = storage::get_plan(&env, sub.plan_id)?;
            token::TokenClient::new(&env, &plan.token).transfer(
                &env.current_contract_address(),
                &sub.subscriber,
                &refund,
            );
        }
        events::status_changed(&env, sub_id, prev, SubscriptionStatus::Cancelled);
        Ok(())
    }

    // ── Keeper action ───────────────────────────────────────────────────────

    /// Attempt to settle the current due period. Permissionless — any
    /// keeper may call it — because every safety property (cap, ceiling,
    /// schedule) is enforced here, not by trusting the caller.
    ///
    /// Payment failure does not error: it returns an outcome and persists
    /// the resulting state transition.
    pub fn charge(env: Env, sub_id: u64) -> Result<ChargeOutcome, Error> {
        let mut sub = storage::get_subscription(&env, sub_id)?;
        if !matches!(
            sub.status,
            SubscriptionStatus::Active | SubscriptionStatus::PastDue
        ) {
            return Err(Error::InvalidStatus);
        }

        let now = env.ledger().timestamp();
        if now < sub.next_charge_at {
            return Err(Error::ChargeNotDue);
        }

        let plan = storage::get_plan(&env, sub.plan_id)?;
        let outcome = Self::settle_period(&env, &plan, &mut sub, now);
        storage::set_subscription(&env, &sub);
        Ok(outcome)
    }

    // ── Read-only queries ───────────────────────────────────────────────────

    pub fn get_plan(env: Env, plan_id: u64) -> Result<Plan, Error> {
        storage::get_plan(&env, plan_id)
    }

    pub fn get_subscription(env: Env, sub_id: u64) -> Result<Subscription, Error> {
        storage::get_subscription(&env, sub_id)
    }

    pub fn get_merchant_plans(env: Env, merchant: Address) -> Vec<u64> {
        storage::get_merchant_plans(&env, &merchant)
    }

    pub fn get_subscriber_subscriptions(env: Env, subscriber: Address) -> Vec<u64> {
        storage::get_subscriber_subs(&env, &subscriber)
    }

    /// Prepaid balance currently held for a subscription.
    pub fn get_balance(env: Env, sub_id: u64) -> Result<i128, Error> {
        Ok(storage::get_subscription(&env, sub_id)?.balance)
    }

    /// Whether a charge is currently permitted for this subscription.
    pub fn is_charge_due(env: Env, sub_id: u64) -> Result<bool, Error> {
        let sub = storage::get_subscription(&env, sub_id)?;
        Ok(matches!(
            sub.status,
            SubscriptionStatus::Active | SubscriptionStatus::PastDue
        ) && env.ledger().timestamp() >= sub.next_charge_at)
    }

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        storage::get_admin(&env)
    }

    /// Contract-level version constant.
    pub fn get_version(_env: Env) -> u32 {
        CONTRACT_VERSION
    }

    // ── Internal ────────────────────────────────────────────────────────────

    /// Core billing step shared by `subscribe` (first period) and
    /// `charge` (every later period). Mutates `sub` in place and emits
    /// the matching events; the caller persists.
    fn settle_period(env: &Env, plan: &Plan, sub: &mut Subscription, now: u64) -> ChargeOutcome {
        let prev = sub.status;

        // Defensive: a plan is immutable, but re-check the cap so the
        // invariant "never charge above max_amount" is enforced at the
        // exact point value moves, not only at subscribe time.
        debug_assert!(plan.amount <= sub.max_amount);

        // Lifetime ceiling reached → complete as Expired, charge nothing.
        if sub.spend_ceiling > 0 && sub.total_spent + plan.amount > sub.spend_ceiling {
            sub.status = SubscriptionStatus::Expired;
            events::status_changed(env, sub.id, prev, sub.status);
            return ChargeOutcome::CeilingReached;
        }

        if sub.balance >= plan.amount {
            // Happy path: move one period's price to the merchant.
            sub.balance -= plan.amount;
            sub.total_spent += plan.amount;
            sub.periods_paid += 1;
            sub.failed_attempts = 0;
            // Recovery from PastDue re-anchors the schedule at `now`
            // (no retroactive back-billing of missed periods in v1).
            sub.next_charge_at = if prev == SubscriptionStatus::PastDue {
                now + plan.interval
            } else {
                sub.next_charge_at + plan.interval
            };
            sub.status = SubscriptionStatus::Active;

            token::TokenClient::new(env, &plan.token).transfer(
                &env.current_contract_address(),
                &plan.merchant,
                &plan.amount,
            );
            events::charged(env, sub.id, plan.amount, sub.periods_paid);
            if prev != SubscriptionStatus::Active {
                events::status_changed(env, sub.id, prev, sub.status);
            }
            ChargeOutcome::Charged
        } else if now > sub.next_charge_at.saturating_add(plan.grace_period) {
            // Underfunded past the grace window → terminal Expired.
            // Any residual balance is refunded to the subscriber here,
            // since terminal states hold no funds.
            let refund = sub.balance;
            sub.balance = 0;
            sub.status = SubscriptionStatus::Expired;
            if refund > 0 {
                token::TokenClient::new(env, &plan.token).transfer(
                    &env.current_contract_address(),
                    &sub.subscriber,
                    &refund,
                );
            }
            events::status_changed(env, sub.id, prev, sub.status);
            ChargeOutcome::GraceElapsed
        } else {
            // Underfunded but within grace → PastDue, count the attempt.
            sub.status = SubscriptionStatus::PastDue;
            sub.failed_attempts += 1;
            events::charge_failed(env, sub.id, prev, sub.status, sub.failed_attempts);
            ChargeOutcome::InsufficientFunds
        }
    }
}
