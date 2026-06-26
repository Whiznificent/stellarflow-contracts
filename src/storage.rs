//! Storage utilities for consumer lease management and rent safety.

use soroban_sdk::{symbol_short, Env, Symbol, Address};

const LEASE_KEY: Symbol = symbol_short!("LEASE");

/// Minimum remaining TTL ledgers before auto-extending.
const RENT_SAFETY_THRESHOLD: u32 = 5_000;
/// Target TTL ledgers when extending (~24 h at 5 s/ledger).
const RENT_EXTEND_TO: u32 = 17_280;

/// Pre-flight rent verification hook.
/// Checks remaining instance storage lifetime and extends if below safety bounds.
/// Call before writing new data rows to avoid mid-execution rent failures.
pub fn preflight_rent_check(env: &Env) {
    let ttl = env.storage().instance().get_ttl();
    if ttl < RENT_SAFETY_THRESHOLD {
        env.storage().instance().extend_ttl(RENT_SAFETY_THRESHOLD, RENT_EXTEND_TO);
    }
}

/// Extend the lease for a given consumer based on interaction frequency.
pub fn extend_consumer_lease(env: &Env, consumer: &Address, frequency: u64) {
    const EXTENSION_FACTOR: u64 = 10;
    let mut lease: u64 = env
        .storage()
        .instance()
        .get(&(LEASE_KEY, consumer.clone()))
        .unwrap_or(0);
    let addition = frequency.saturating_mul(EXTENSION_FACTOR);
    lease = lease.saturating_add(addition);
    env.storage()
        .instance()
        .set(&(LEASE_KEY, consumer.clone()), &lease);
}
