use crate::{AssetId, ContractError, TimeLockedUpgradeContract};
use soroban_sdk::{contracttype, Address, Env, Vec};

pub const STANDARD_FIXED_POINT_SCALE: i128 = 10_000_000;
pub const INTERIOR_FEE_PRECISION_SCALE: i128 = 100_000_000_000_000;

#[contracttype]
#[derive(Clone)]
pub struct CorridorFeePool {
    pub asset: AssetId,
    pub collected: u64,
    pub variable_pool: u64,
}

#[contracttype]
pub enum FeesStorageKey {
    CorridorPool(AssetId),
}

impl CorridorFeePool {
    fn new(asset: AssetId) -> Self {
        Self {
            asset,
            collected: 0,
            variable_pool: 0,
        }
    }
}

pub fn add_corridor_fees(
    env: Env,
    admin: Address,
    asset: AssetId,
    collected: u64,
    variable_fee: u64,
) -> Result<CorridorFeePool, ContractError> {
    admin.require_auth();
    let data = TimeLockedUpgradeContract::get_data(&env)?;
    if data.admin != admin {
        return Err(ContractError::NotAdmin);
    }

    let key = FeesStorageKey::CorridorPool(asset.clone());
    let mut pool: CorridorFeePool = env
        .storage()
        .instance()
        .get(&key)
        .unwrap_or(CorridorFeePool::new(asset.clone()));

    pool.collected = pool
        .collected
        .checked_add(collected)
        .ok_or(ContractError::Overflow)?;
    pool.variable_pool = pool
        .variable_pool
        .checked_add(variable_fee)
        .ok_or(ContractError::Overflow)?;

    env.storage().instance().set(&key, &pool);
    Ok(pool)
}

pub fn get_corridor_fee_pool(env: Env, asset: AssetId) -> CorridorFeePool {
    let key = FeesStorageKey::CorridorPool(asset.clone());
    env.storage()
        .instance()
        .get(&key)
        .unwrap_or(CorridorFeePool::new(asset))
}

pub fn distribute_variable_fee_pool(
    env: &Env,
    variable_pool: u64,
    relayer_weights: Vec<u64>,
) -> Result<Vec<u64>, ContractError> {
    let total_weight = relayer_weights
        .iter()
        .try_fold(0_i128, |acc, weight| {
            acc.checked_add(weight as i128)
                .ok_or(ContractError::Overflow)
        })?;

    let mut profiles = Vec::new(env);
    if total_weight == 0 || relayer_weights.len() == 0 {
        return Ok(profiles);
    }

    let pool_profile = (variable_pool as i128)
        .checked_mul(STANDARD_FIXED_POINT_SCALE)
        .ok_or(ContractError::Overflow)?;
    let interior_pool_profile = pool_profile
        .checked_mul(INTERIOR_FEE_PRECISION_SCALE)
        .ok_or(ContractError::Overflow)?;

    let last_index = relayer_weights.len() - 1;
    let mut assigned_profile = 0_i128;

    for index in 0..relayer_weights.len() {
        let profile = if index == last_index {
            pool_profile
                .checked_sub(assigned_profile)
                .ok_or(ContractError::Overflow)?
        } else {
            let weight = relayer_weights
                .get(index)
                .ok_or(ContractError::Overflow)? as i128;
            let interior_share = interior_pool_profile
                .checked_mul(weight)
                .ok_or(ContractError::Overflow)?
                .checked_div(total_weight)
                .ok_or(ContractError::DivisionByZero)?;
            interior_share
                .checked_div(INTERIOR_FEE_PRECISION_SCALE)
                .ok_or(ContractError::DivisionByZero)?
        };

        assigned_profile = assigned_profile
            .checked_add(profile)
            .ok_or(ContractError::Overflow)?;
        profiles.push_back(profile.try_into().map_err(|_| ContractError::Overflow)?);
    }

    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_distribution_normalizes_to_standard_fixed_point_footprint() {
        let env = Env::default();
        let mut weights = Vec::new(&env);
        weights.push_back(1);
        weights.push_back(1);
        weights.push_back(1);

        let profiles = distribute_variable_fee_pool(&env, 1, weights).unwrap();

        assert_eq!(profiles.get(0), Some(3_333_333));
        assert_eq!(profiles.get(1), Some(3_333_333));
        assert_eq!(profiles.get(2), Some(3_333_334));
        assert_eq!(
            profiles.iter().fold(0_u64, |acc, value| acc + value),
            STANDARD_FIXED_POINT_SCALE as u64
        );
    }

    #[test]
    fn fee_distribution_preserves_fractional_weight_balance() {
        let env = Env::default();
        let mut weights = Vec::new(&env);
        weights.push_back(2);
        weights.push_back(3);
        weights.push_back(5);

        let profiles = distribute_variable_fee_pool(&env, 7, weights).unwrap();

        assert_eq!(profiles.get(0), Some(14_000_000));
        assert_eq!(profiles.get(1), Some(21_000_000));
        assert_eq!(profiles.get(2), Some(35_000_000));
        assert_eq!(profiles.iter().fold(0_u64, |acc, value| acc + value), 70_000_000);
    }

    #[test]
    fn fee_distribution_rejects_overflow_before_division() {
        let env = Env::default();
        let mut weights = Vec::new(&env);
        weights.push_back(u64::MAX);

        let result = distribute_variable_fee_pool(&env, u64::MAX, weights);

        assert_eq!(result, Err(ContractError::Overflow));
    }
}
