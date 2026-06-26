//! Liquidity volume validation module — flash loan manipulation prevention.
//!
//! Aggregating market prices from thinly backed liquidity channels can expose
//! downstream financial engines to flash loan price manipulations. This module
//! implements explicit liquidity volume validation checks that terminate
//! transaction paths early if a validator node's reported pool liquidity falls
//! below the configured minimum security threshold.
//!
//! # Security Model
//! 
//! Flash loan attacks exploit temporary price dislocations in low-liquidity pools.
//! By requiring minimum liquidity thresholds, we ensure that price submissions
//! come from markets with sufficient depth to resist manipulation.
//!
//! # Flow
//! 1. Admin sets liquidity threshold per asset via `set_liquidity_threshold`.
//! 2. Provider submits price + liquidity data via `update_price`.
//! 3. Contract validates liquidity meets threshold before accepting submission.
//! 4. Submissions below threshold are rejected with `LiquidityBelowThreshold` error.
//!
//! # Storage layout
//! | Key                                  | Type      | Description                                    |
//! |--------------------------------------|-----------|------------------------------------------------|
//! | `DataKey::LiquidityThreshold(Symbol)` | `i128`    | Minimum liquidity required per asset (stroops) |
//! | `DataKey::ProviderReportedLiquidity(Address, Symbol)` | `i128` | Last reported liquidity by provider for asset |
//! | `DataKey::LastLiquidityValidation(Symbol)` | `u64` | Timestamp of last successful validation |

use soroban_sdk::{Address, Env, Symbol};

use crate::types::DataKey;
use crate::ContractError;

/// Minimum allowed liquidity threshold (1 XLM equivalent = 10_000_000 stroops).
/// Prevents admins from setting unreasonably low thresholds that defeat the purpose.
pub const MIN_LIQUIDITY_THRESHOLD: i128 = 10_000_000;

/// Maximum reasonable liquidity threshold (1 billion XLM equivalent).
/// Prevents accidental misconfiguration that would reject all submissions.
pub const MAX_LIQUIDITY_THRESHOLD: i128 = 1_000_000_000_0000000;

/// Multiplier for low-liquidity slash penalty (basis points).
/// Applied when provider submits prices from pools below the threshold.
pub const LOW_LIQUIDITY_SLASH_MULTIPLIER: i128 = 5;

// ─────────────────────────────────────────────────────────────────────────────
// Storage Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read the minimum liquidity threshold for an asset.
/// Returns None if no threshold has been configured.
pub fn get_liquidity_threshold(env: &Env, asset: &Symbol) -> Option<i128> {
    env.storage()
        .persistent()
        .get(&DataKey::LiquidityThreshold(asset.clone()))
}

/// Set the minimum liquidity threshold for an asset.
/// Must be within MIN_LIQUIDITY_THRESHOLD..MAX_LIQUIDITY_THRESHOLD range.
fn set_liquidity_threshold(env: &Env, asset: &Symbol, threshold: i128) {
    env.storage()
        .persistent()
        .set(&DataKey::LiquidityThreshold(asset.clone()), &threshold);
}

/// Read the last reported liquidity from a specific provider for an asset.
/// Returns None if the provider has never reported liquidity for this asset.
pub fn get_provider_liquidity(env: &Env, provider: &Address, asset: &Symbol) -> Option<i128> {
    env.storage()
        .persistent()
        .get(&DataKey::ProviderReportedLiquidity(
            provider.clone(),
            asset.clone(),
        ))
}

/// Store the liquidity value reported by a provider for an asset.
fn set_provider_liquidity(env: &Env, provider: &Address, asset: &Symbol, liquidity: i128) {
    env.storage().persistent().set(
        &DataKey::ProviderReportedLiquidity(provider.clone(), asset.clone()),
        &liquidity,
    );
}

/// Record the timestamp of the last successful liquidity validation for an asset.
fn set_last_validation_timestamp(env: &Env, asset: &Symbol) {
    let timestamp = env.ledger().timestamp();
    env.storage()
        .persistent()
        .set(&DataKey::LastLiquidityValidation(asset.clone()), &timestamp);
}

/// Read the timestamp of the last successful liquidity validation for an asset.
pub fn get_last_validation_timestamp(env: &Env, asset: &Symbol) -> Option<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::LastLiquidityValidation(asset.clone()))
}

/// Ensure the current consensus buffer contains at least three independent
/// provider sources before a price can be finalized.
///
/// This prevents accepting a finalized consensus price when the active input
/// pool has collapsed below the minimum safe participation threshold.
pub fn validate_consensus_quorum(env: &Env, buffer: &crate::types::PriceBuffer) -> Result<(), ContractError> {
    let mut unique_sources = soroban_sdk::Map::new(env);

    for entry in buffer.entries.iter() {
        unique_sources.set(entry.provider.clone(), ());
    }

    if unique_sources.len() < 3 {
        return Err(ContractError::IncompleteQuorum);
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Core Validation Logic
// ─────────────────────────────────────────────────────────────────────────────

/// Validate that reported pool liquidity meets the configured minimum threshold.
///
/// This function is called during `update_price` to ensure price submissions
/// come from sufficiently liquid markets that cannot be easily manipulated via
/// flash loans or other short-term capital injection attacks.
///
/// # Parameters
/// - `env`: Soroban environment
/// - `asset`: The asset pair being priced (e.g. "XLM/USD")
/// - `provider`: Address of the relayer submitting the price
/// - `reported_liquidity`: Total pool liquidity value reported by the provider (in stroops)
///
/// # Returns
/// - `Ok(())` if liquidity meets or exceeds the threshold, or no threshold is set
/// - `Err(ContractError::LiquidityBelowThreshold)` if liquidity is insufficient
/// - `Err(ContractError::InvalidLiquidity)` if reported liquidity is negative or zero
///
/// # Security Properties
/// 1. **Early termination**: Transaction is rejected before price enters buffer
/// 2. **Per-asset thresholds**: Different assets can have different liquidity requirements
/// 3. **Provider tracking**: Historical liquidity data enables reputation scoring
/// 4. **Audit trail**: Timestamps allow reconstruction of liquidity history
///
/// # Example
/// ```rust
/// // Admin sets 100M stroops minimum liquidity for XLM/USD
/// set_liquidity_threshold_internal(&env, &Symbol::new(&env, "XLM_USD"), 100_000_000);
///
/// // Provider attempts to submit price with 50M liquidity
/// let result = validate_liquidity(
///     &env,
///     &Symbol::new(&env, "XLM_USD"),
///     &provider_addr,
///     50_000_000
/// );
/// // Result: Err(ContractError::LiquidityBelowThreshold)
/// ```
pub fn validate_liquidity(
    env: &Env,
    asset: &Symbol,
    provider: &Address,
    reported_liquidity: i128,
) -> Result<(), ContractError> {
    // Reject negative or zero liquidity values
    if reported_liquidity <= 0 {
        return Err(ContractError::InvalidLiquidity);
    }

    // Check if a liquidity threshold has been configured for this asset
    let threshold = match get_liquidity_threshold(env, asset) {
        Some(t) => t,
        None => {
            // No threshold configured — validation passes by default.
            // This allows gradual rollout: assets without explicit thresholds
            // continue to accept all submissions until governance configures them.
            return Ok(());
        }
    };

    // Compare reported liquidity against the configured threshold
    if reported_liquidity < threshold {
        // Emit event for monitoring and alerting
        env.events().publish(
            (Symbol::new(env, "liquidity_violation"),),
            (
                asset.clone(),
                provider.clone(),
                reported_liquidity,
                threshold,
            ),
        );

        // Store the insufficient liquidity value for reputation tracking
        set_provider_liquidity(env, provider, asset, reported_liquidity);

        return Err(ContractError::LiquidityBelowThreshold);
    }

    // Validation passed — record the successful submission
    set_provider_liquidity(env, provider, asset, reported_liquidity);
    set_last_validation_timestamp(env, asset);

    // Emit success event for monitoring
    env.events().publish(
        (Symbol::new(env, "liquidity_validated"),),
        (
            asset.clone(),
            provider.clone(),
            reported_liquidity,
            threshold,
        ),
    );

    Ok(())
}

/// Compute the weighted index price from a borrowed basket of assets.
pub fn calculate_index_price(
    env: &Env,
    components: &Vec<AssetWeight>,
) -> Result<i128, ContractError> {
    if components.is_empty() {
        return Err(ContractError::AssetNotFound);
    }

    let mut total_weighted_price: i128 = 0;
    let mut total_weight: u32 = 0;

    for component in components.iter() {
        if !env
            .storage()
            .persistent()
            .has(&DataKey::TrackedAsset(component.asset.clone()))
        {
            return Err(ContractError::AssetNotFound);
        }

        if component.weight == 0 {
            return Err(ContractError::InvalidWeight);
        }

        let price_data = crate::PriceOracle::get_price(env.clone(), component.asset.clone(), true)?;
        let weight_i128: i128 = component.weight.into();
        let weighted_val = price_data
            .price
            .checked_mul(weight_i128)
            .ok_or(ContractError::InvalidPrice)?;

        total_weighted_price = total_weighted_price
            .checked_add(weighted_val)
            .ok_or(ContractError::InvalidPrice)?;

        total_weight = total_weight
            .checked_add(component.weight)
            .unwrap_or(total_weight);
    }

    if total_weight == 0 {
        return Err(ContractError::InvalidWeight);
    }

    total_weighted_price
        .checked_div(total_weight as i128)
        .ok_or(ContractError::PriceMathOverflow)
}

/// Remove a batch of price entries without copying the input vector.
pub fn clear_assets(env: &Env, assets: &Vec<Symbol>) -> Result<(), ContractError> {
    if assets.len() > MAX_CLEAR_ASSETS {
        return Err(ContractError::TooManyAssets);
    }

    let storage = env.storage().persistent();
    for asset in assets.iter() {
        storage.remove(&DataKey::Price(asset));
    }

    Ok(())
}
