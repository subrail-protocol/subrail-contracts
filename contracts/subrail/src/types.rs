use soroban_sdk::{contracttype, Address, String};

/// Lifecycle of a subscription.
///
///  ⁠text
///                          +---------+
///        subscribe ------> | Active  | <---- resume / successful charge
///                          +---------+
///                            |  |  |
///          charge failed --- |  |  --- pause
///                            v  |         v
///                       +---------+   +--------+
///                       | PastDue |   | Paused |
///                       +---------+   +--------+
///                            |
///        grace elapsed ------+------> Expired
///
///        cancel (from Active / PastDue / Paused) ------> Cancelled
///
⁠ #[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    /// Billing normally.
    Active = 0,
    /// Last charge attempt failed; within the grace window.
    PastDue = 1,
    /// Voluntarily paused by the subscriber; no charges occur.
    Paused = 2,
    /// Terminated by the subscriber; terminal state.
    Cancelled = 3,
    /// Terminated by the protocol (grace elapsed or spend ceiling
    /// reached); terminal state.
    Expired = 4,
}

/// Outcome of a `charge` attempt. `charge` deliberately returns an
/// outcome instead of erroring on payment failure, so that state
/// transitions (PastDue, Expired) persist when a keeper calls it.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ChargeOutcome {
    /// Payment moved to the merchant; subscription remains Active.
    Charged = 0,
    /// Deposited balance could not cover the period; now PastDue.
    InsufficientFunds = 1,
    /// Grace window elapsed without recovery; now Expired.
    GraceElapsed = 2,
    /// Spend ceiling reached; subscription completed as Expired.
    CeilingReached = 3,
}

/// A merchant's billing plan. Immutable after creation except for the
/// `active` flag, so subscribers always know exactly what they agreed to.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    /// Sequential plan identifier.
    pub id: u64,
    /// Account that receives charges and controls the plan.
    pub merchant: Address,
    /// SEP-41 token contract used for billing (e.g. USDC SAC).
    pub token: Address,
    /// Price per billing period, in the token's stroops/decimals.
    pub amount: i128,
    /// Billing period length in seconds.
    pub interval: u64,
    /// Seconds after a missed charge before the subscription expires.
    pub grace_period: u64,
    /// Human-readable plan name.
    pub name: String,
    /// Whether the plan accepts new subscriptions.
    pub active: bool,
    /// Ledger timestamp at creation.
    pub created_at: u64,
}

/// A subscriber's authorization against a plan, together with the
/// prepaid balance the protocol may draw from.
///
/// SubRail uses a deposit model: the subscriber funds a balance held by
/// this contract, and `charge` moves at most `plan.amount` per elapsed
/// period from that balance to the merchant — never more, never early.
/// The subscriber can withdraw the unused balance at any time.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    /// Sequential subscription identifier.
    pub id: u64,
    /// The plan this subscription is bound to.
    pub plan_id: u64,
    /// The paying account.
    pub subscriber: Address,
    /// Current lifecycle state.
    pub status: SubscriptionStatus,
    /// Hard per-period cap authorized by the subscriber. A charge can
    /// never exceed this, regardless of the plan.
    pub max_amount: i128,
    /// Optional lifetime spend ceiling. `0` means unlimited. Once
    /// `total_spent + plan.amount` would exceed it, the subscription
    /// completes as Expired instead of charging.
    pub spend_ceiling: i128,
    /// Prepaid balance held by the contract for this subscription.
    pub balance: i128,
    /// Cumulative amount ever charged to the merchant.
    pub total_spent: i128,
    /// Timestamp at/after which the next charge is permitted.
    pub next_charge_at: u64,
    /// Number of successfully billed periods.
    pub periods_paid: u32,
    /// Consecutive failed charge attempts since the last success.
    pub failed_attempts: u32,
    /// Timestamp of pause, `0` when not paused. Used to shift the
    /// schedule by the paused duration on resume.
    pub paused_at: u64,
    /// Ledger timestamp at creation.
    pub created_at: u64,
}
