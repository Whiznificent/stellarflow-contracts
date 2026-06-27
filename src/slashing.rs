//! Multi-tier escrow penalties for repetitive ingestion drops (Issue #525).
//!
//! Maintains a progressive penalty tracking matrix per (validator, asset feed)
//! pair. Occasional connectivity blips incur a base bond deduction, while
//! repeated outages inside a rolling 100-ledger window scale deductions
//! exponentially to discourage persistent feed negligence.

use soroban_sdk::{contracttype, Address, Env, Map, Symbol, Vec};

use crate::ContractError;

/// Rolling ledger window for tracking repeated ingestion dropouts.
pub const ROLLING_FAULT_WINDOW_LEDGERS: u32 = 100;

/// Maximum exponential multiplier (2^10) to prevent unbounded bond seizure.
pub const MAX_PENALTY_MULTIPLIER: u64 = 1_024;

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct TrackingFaultHistory {
    /// Ledger sequences when ingestion dropouts were recorded.
    pub fault_ledgers: Vec<u32>,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum SlashingStorageKey {
    FaultHistory(Address, Symbol),
}

/// Result of applying a progressive escrow bond deduction.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct IngestionPenaltyResult {
    pub validator: Address,
    pub asset: Symbol,
    pub fault_count: u32,
    pub penalty_multiplier: u64,
    pub bond_deduction: u64,
    pub remaining_stake: u64,
}

/// Exponential multiplier: 2^(fault_count - 1), minimum 1, capped at 1024.
pub fn get_penalty_multiplier(fault_count: u32) -> u64 {
    if fault_count == 0 {
        return 1;
    }

    let shift = fault_count.saturating_sub(1).min(10);
    let multiplier = 1u64.checked_shl(shift).unwrap_or(MAX_PENALTY_MULTIPLIER);
    multiplier.min(MAX_PENALTY_MULTIPLIER)
}

/// Compute the structural bond deduction for a fault count and base bond amount.
pub fn calculate_bond_deduction(base_bond: u64, fault_count: u32) -> Result<u64, ContractError> {
    if base_bond == 0 {
        return Err(ContractError::InvalidStakeAmount);
    }

    let multiplier = get_penalty_multiplier(fault_count);
    base_bond
        .checked_mul(multiplier)
        .ok_or(ContractError::Overflow)
}

/// Count faults whose ledger sequence falls within the rolling window.
pub fn count_faults_in_window(history: &TrackingFaultHistory, current_ledger: u32) -> u32 {
    let window_start = current_ledger.saturating_sub(ROLLING_FAULT_WINDOW_LEDGERS);
    let mut count = 0u32;

    for i in 0..history.fault_ledgers.len() {
        let ledger = history.fault_ledgers.get(i).unwrap_or(0);
        if ledger >= window_start && ledger <= current_ledger {
            count = count.saturating_add(1);
        }
    }

    count
}

/// Drop fault entries that fell outside the rolling ledger window.
pub fn prune_fault_history(env: &Env, history: &mut TrackingFaultHistory, current_ledger: u32) {
    let window_start = current_ledger.saturating_sub(ROLLING_FAULT_WINDOW_LEDGERS);
    let mut retained = Vec::new(env);

    for i in 0..history.fault_ledgers.len() {
        let ledger = history.fault_ledgers.get(i).unwrap_or(0);
        if ledger >= window_start && ledger <= current_ledger {
            retained.push_back(ledger);
        }
    }

    history.fault_ledgers = retained;
}

fn load_fault_history(env: &Env, validator: &Address, asset: &Symbol) -> TrackingFaultHistory {
    env.storage()
        .persistent()
        .get(&SlashingStorageKey::FaultHistory(validator.clone(), asset.clone()))
        .unwrap_or(TrackingFaultHistory {
            fault_ledgers: Vec::new(env),
        })
}

fn store_fault_history(env: &Env, validator: &Address, asset: &Symbol, history: &TrackingFaultHistory) {
    env.storage().persistent().set(
        &SlashingStorageKey::FaultHistory(validator.clone(), asset.clone()),
        history,
    );
}

/// Record an ingestion dropout and return the active fault count in the window.
pub fn record_tracking_fault(
    env: &Env,
    validator: &Address,
    asset: &Symbol,
) -> Result<u32, ContractError> {
    let current_ledger = env.ledger().sequence();
    let mut history = load_fault_history(env, validator, asset);

    prune_fault_history(env, &mut history, current_ledger);
    history.fault_ledgers.push_back(current_ledger);
    store_fault_history(env, validator, asset, &history);

    Ok(count_faults_in_window(&history, current_ledger))
}

/// Read the active fault count without recording a new dropout.
pub fn get_fault_count_in_window(env: &Env, validator: &Address, asset: &Symbol) -> u32 {
    let current_ledger = env.ledger().sequence();
    let mut history = load_fault_history(env, validator, asset);
    prune_fault_history(env, &mut history, current_ledger);
    count_faults_in_window(&history, current_ledger)
}

/// Deduct the exponentially scaled bond from validator stake registries.
pub fn apply_escrow_penalty(
    env: &Env,
    validator: &Address,
    asset: &Symbol,
    base_bond: u64,
    fault_count: u32,
    stake_registry_key: &Symbol,
    total_staked_key: &Symbol,
    feed_stake_key: &crate::StakingStorageKey,
) -> Result<IngestionPenaltyResult, ContractError> {
    let bond_deduction = calculate_bond_deduction(base_bond, fault_count)?;
    let penalty_multiplier = get_penalty_multiplier(fault_count);

    let mut stakes: Map<Address, u64> = env
        .storage()
        .instance()
        .get(stake_registry_key)
        .unwrap_or_else(|| Map::new(env));

    let node_total = stakes.get(validator.clone()).unwrap_or(0);
    if node_total == 0 {
        return Err(ContractError::InsufficientBondForPenalty);
    }

    let actual_deduction = bond_deduction.min(node_total);
    let remaining_stake = node_total - actual_deduction;

    if remaining_stake == 0 {
        stakes.remove(validator.clone());
    } else {
        stakes.set(validator.clone(), remaining_stake);
    }

    let total: u64 = env.storage().instance().get(total_staked_key).unwrap_or(0);
    let new_total = total.saturating_sub(actual_deduction);

    env.storage().instance().set(stake_registry_key, &stakes);
    env.storage().instance().set(total_staked_key, &new_total);

    if let Some(feed_stake) = env.storage().persistent().get::<_, u64>(feed_stake_key) {
        let feed_remaining = feed_stake.saturating_sub(actual_deduction);
        if feed_remaining == 0 {
            env.storage().persistent().remove(feed_stake_key);
        } else {
            env.storage().persistent().set(feed_stake_key, &feed_remaining);
        }
    }

    Ok(IngestionPenaltyResult {
        validator: validator.clone(),
        asset: asset.clone(),
        fault_count,
        penalty_multiplier,
        bond_deduction: actual_deduction,
        remaining_stake,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    fn sample_history(env: &Env, ledgers: &[u32]) -> TrackingFaultHistory {
        let mut fault_ledgers = Vec::new(env);
        for ledger in ledgers {
            fault_ledgers.push_back(*ledger);
        }
        TrackingFaultHistory { fault_ledgers }
    }

    #[test]
    fn first_outage_uses_base_multiplier() {
        assert_eq!(get_penalty_multiplier(1), 1);
        assert_eq!(calculate_bond_deduction(500, 1).unwrap(), 500);
    }

    #[test]
    fn repeated_outages_scale_exponentially() {
        assert_eq!(get_penalty_multiplier(2), 2);
        assert_eq!(get_penalty_multiplier(3), 4);
        assert_eq!(get_penalty_multiplier(4), 8);
        assert_eq!(calculate_bond_deduction(100, 4).unwrap(), 800);
    }

    #[test]
    fn multiplier_is_capped() {
        assert_eq!(get_penalty_multiplier(20), MAX_PENALTY_MULTIPLIER);
    }

    #[test]
    fn rolling_window_excludes_old_faults() {
        let env = Env::default();
        let history = sample_history(&env, &[50, 80, 150, 200]);
        assert_eq!(count_faults_in_window(&history, 200), 2);
        assert_eq!(count_faults_in_window(&history, 150), 3);
    }

    #[test]
    fn prune_drops_faults_outside_window() {
        let env = Env::default();
        let mut history = sample_history(&env, &[10, 50, 120, 200]);
        prune_fault_history(&env, &mut history, 200);
        assert_eq!(history.fault_ledgers.len(), 2);
        assert_eq!(history.fault_ledgers.get(0).unwrap(), 120);
    }

    #[test]
    fn apply_penalty_scales_with_repeat_outages() {
        use soroban_sdk::{contract, contractimpl, symbol_short, testutils::Address as _, Map};
        use crate::StakingStorageKey;

        #[contract]
        struct SlashHarness;

        #[contractimpl]
        impl SlashHarness {}

        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, SlashHarness);
        let validator = Address::generate(&env);
        let asset = symbol_short!("NGN");
        let stake_key = symbol_short!("STAKES");
        let total_key = symbol_short!("TOTAL");

        env.as_contract(&contract_id, || {
            let mut stakes = Map::new(&env);
            stakes.set(validator.clone(), 10_000u64);
            env.storage().instance().set(&stake_key, &stakes);
            env.storage().instance().set(&total_key, &10_000u64);

            record_tracking_fault(&env, &validator, &asset).unwrap();
            let first = apply_escrow_penalty(
                &env,
                &validator,
                &asset,
                100,
                1,
                &stake_key,
                &total_key,
                &StakingStorageKey::FeedStake(validator.clone(), asset.clone()),
            )
            .unwrap();
            assert_eq!(first.bond_deduction, 100);
            assert_eq!(first.penalty_multiplier, 1);

            record_tracking_fault(&env, &validator, &asset).unwrap();
            let second = apply_escrow_penalty(
                &env,
                &validator,
                &asset,
                100,
                2,
                &stake_key,
                &total_key,
                &StakingStorageKey::FeedStake(validator.clone(), asset.clone()),
            )
            .unwrap();
            assert_eq!(second.bond_deduction, 200);
            assert_eq!(second.penalty_multiplier, 2);

            record_tracking_fault(&env, &validator, &asset).unwrap();
            let third = apply_escrow_penalty(
                &env,
                &validator,
                &asset,
                100,
                3,
                &stake_key,
                &total_key,
                &StakingStorageKey::FeedStake(validator.clone(), asset.clone()),
            )
            .unwrap();
            assert_eq!(third.bond_deduction, 400);
            assert_eq!(third.penalty_multiplier, 4);
        });
    }

    #[test]
    fn faults_outside_window_do_not_increase_multiplier() {
        use soroban_sdk::{contract, contractimpl, symbol_short, testutils::Address as _};
        use soroban_sdk::testutils::Ledger;

        #[contract]
        struct WindowHarness;

        #[contractimpl]
        impl WindowHarness {}

        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, WindowHarness);
        let validator = Address::generate(&env);
        let asset = symbol_short!("KES");

        env.as_contract(&contract_id, || {
            let mut history = TrackingFaultHistory {
                fault_ledgers: Vec::new(&env),
            };
            history.fault_ledgers.push_back(10);
            env.storage().persistent().set(
                &SlashingStorageKey::FaultHistory(validator.clone(), asset.clone()),
                &history,
            );

            env.ledger().set(soroban_sdk::testutils::LedgerInfo {
                timestamp: env.ledger().timestamp(),
                protocol_version: env.ledger().protocol_version(),
                sequence_number: 200,
                network_id: Default::default(),
                base_reserve: 10,
                min_temp_entry_ttl: 0,
                min_persistent_entry_ttl: 0,
                max_entry_ttl: u32::MAX,
            });
            let count = record_tracking_fault(&env, &validator, &asset).unwrap();
            assert_eq!(count, 1);
            assert_eq!(get_penalty_multiplier(count), 1);
        });
    }
}
