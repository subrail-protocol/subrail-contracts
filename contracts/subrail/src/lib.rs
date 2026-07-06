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

use soroban_sdk::{contract, contractimpl, Address, Env};

pub use crate::errors::Error;
pub use crate::types::{ChargeOutcome, Plan, Subscription, SubscriptionStatus};

// ── Validation constants ────────────────────────────────────────────────

/// Contract-level version, surfaced by `get_version()`.
const CONTRACT_VERSION: u32 = 1;

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

    // ── Read-only queries ───────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        storage::get_admin(&env)
    }

    /// Contract-level version constant.
    pub fn get_version(_env: Env) -> u32 {
        CONTRACT_VERSION
    }
}
