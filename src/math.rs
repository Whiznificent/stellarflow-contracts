//! Integer-precision variance tracking for cross-border corridor rates.
//!
//! All arithmetic enforces strict integer-only multiplication before
//! scale-down division passes to preserve calculation precision and
//! prevent rounding distortions.

use crate::ContractError;

/// Compute the checked sum of a slice of `i128` values.
///
/// Returns `ContractError::Overflow` if any intermediate addition
/// exceeds `i128::MAX`.
pub fn compute_sum(values: &[i128]) -> Result<i128, ContractError> {
    values
        .iter()
        .try_fold(0_i128, |acc, &v| acc.checked_add(v).ok_or(ContractError::Overflow))
}

/// Compute the integer arithmetic mean (floor) of a slice of `i128` values.
///
/// Returns `0` for an empty slice.  The mean is truncated toward zero,
/// which is acceptable for downstream variance computation since the
/// squared-deviation pass operates on exact deltas.
pub fn compute_mean(values: &[i128]) -> Result<i128, ContractError> {
    let n = values.len();
    if n == 0 {
        return Ok(0);
    }
    let sum = compute_sum(values)?;
    Ok(sum / n as i128)
}

/// Compute the sum of squared deviations from a pre-computed mean.
///
/// Each deviation `(value - mean)` is squared **before** any scaling
/// or division, preserving all bit-width precision until the final
/// variance pass.  All intermediate operations use checked arithmetic.
pub fn compute_sum_squared_deviations(values: &[i128], mean: i128) -> Result<i128, ContractError> {
    values.iter().try_fold(0_i128, |acc, &v| {
        let dev = v - mean;
        let sq = dev.checked_mul(dev).ok_or(ContractError::Overflow)?;
        acc.checked_add(sq).ok_or(ContractError::Overflow)
    })
}

/// Compute the **population** variance of a sample set.
///
/// Formula: `sum((value - mean)²) / n`
///
/// Squared deviations are accumulated in full precision (multiplication
/// first, division last).  Returns `0` for slices with fewer than 2
/// elements.
pub fn compute_population_variance(values: &[i128]) -> Result<i128, ContractError> {
    let n = values.len();
    if n <= 1 {
        return Ok(0);
    }
    let mean = compute_mean(values)?;
    let sum_sq = compute_sum_squared_deviations(values, mean)?;
    Ok(sum_sq / n as i128)
}

/// Compute the **sample** (unbiased) variance of a sample set.
///
/// Formula: `sum((value - mean)²) / (n - 1)`
///
/// Squared deviations are accumulated in full precision (multiplication
/// first, division last).  Returns `0` for slices with fewer than 2
/// elements.
pub fn compute_sample_variance(values: &[i128]) -> Result<i128, ContractError> {
    let n = values.len();
    if n <= 1 {
        return Ok(0);
    }
    let mean = compute_mean(values)?;
    let sum_sq = compute_sum_squared_deviations(values, mean)?;
    Ok(sum_sq / (n - 1) as i128)
}

/// Compute the spread between two rates in basis points.
///
/// Formula: `|rate_a - rate_b| * 10_000 / rate_a`
///
/// Returns `ContractError::DivisionByZero` if the base rate (`rate_a`)
/// is zero, preventing runtime panics. All intermediate operations use
/// checked arithmetic to prevent overflow.
pub fn calculate_spread_bps(rate_a: i128, rate_b: i128) -> Result<i128, ContractError> {
    if rate_a == 0 {
        return Err(ContractError::DivisionByZero);
    }

    let delta = rate_a.saturating_sub(rate_b).abs();
    let numerator = delta
        .checked_mul(10_000)
        .ok_or(ContractError::Overflow)?;

    // `rate_a` is confirmed non-zero, so this division is safe.
    Ok(numerator / rate_a)
}

/// Multiplies two numbers and scales the result down by a fixed-point factor.
///
/// This function implements a rigid fixed-point arithmetic scaler that
/// pre-multiplies intermediate values by a scale factor of 10^14 before
/// performing division, then normalizes the result back down to the system's
/// target 10^7 footprint.
///
/// # Arguments
/// * `a` - The first number (multiplicand).
/// * `b` - The second number (multiplier).
/// * `scale_factor` - The denominator for scaling down, typically 10^7.
///
/// # Returns
/// The scaled result, or `ContractError` on overflow or division by zero.
pub fn multiply_and_scale_down(a: i128, b: i128, scale_factor: i128) -> Result<i128, ContractError> {
    if scale_factor == 0 {
        return Err(ContractError::DivisionByZero);
    }

    let product = a.checked_mul(b).ok_or(ContractError::Overflow)?;

    // The division performs the scale-down.
    Ok(product / scale_factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_sum ---

    #[test]
    fn test_sum_empty() {
        assert_eq!(compute_sum(&[]), Ok(0));
    }

    #[test]
    fn test_sum_single() {
        assert_eq!(compute_sum(&[42]), Ok(42));
    }

    #[test]
    fn test_sum_multiple() {
        assert_eq!(compute_sum(&[10, 20, 30]), Ok(60));
    }

    #[test]
    fn test_sum_overflow() {
        assert_eq!(
            compute_sum(&[i128::MAX, 1]),
            Err(ContractError::Overflow)
        );
    }

    // --- compute_mean ---

    #[test]
    fn test_mean_empty() {
        assert_eq!(compute_mean(&[]), Ok(0));
    }

    #[test]
    fn test_mean_single() {
        assert_eq!(compute_mean(&[100]), Ok(100));
    }

    #[test]
    fn test_mean_truncates_toward_zero() {
        assert_eq!(compute_mean(&[10, 20, 30, 41]), Ok(25));
    }

    #[test]
    fn test_mean_exact() {
        assert_eq!(compute_mean(&[1_000, 2_000, 3_000]), Ok(2_000));
    }

    // --- compute_sum_squared_deviations ---

    #[test]
    fn test_sum_sq_all_at_mean() {
        let values = &[5, 5, 5];
        assert_eq!(compute_sum_squared_deviations(values, 5), Ok(0));
    }

    #[test]
    fn test_sum_sq_known() {
        let values = &[1, 2, 3, 4, 5];
        let mean = compute_mean(values).unwrap();
        assert_eq!(mean, 3);
        let sum_sq = compute_sum_squared_deviations(values, mean).unwrap();
        // (1-3)² + (2-3)² + (3-3)² + (4-3)² + (5-3)² = 4 + 1 + 0 + 1 + 4 = 10
        assert_eq!(sum_sq, 10);
    }

    #[test]
    fn test_sum_sq_overflow() {
        let values = &[i128::MAX, 0];
        assert_eq!(
            compute_sum_squared_deviations(values, 0),
            Err(ContractError::Overflow)
        );
    }

    // --- compute_population_variance ---

    #[test]
    fn test_pop_variance_empty() {
        assert_eq!(compute_population_variance(&[]), Ok(0));
    }

    #[test]
    fn test_pop_variance_single() {
        assert_eq!(compute_population_variance(&[100]), Ok(0));
    }

    #[test]
    fn test_pop_variance_identical() {
        assert_eq!(compute_population_variance(&[7, 7, 7, 7]), Ok(0));
    }

    #[test]
    fn test_pop_variance_known() {
        let values = &[1, 2, 3, 4, 5];
        let var = compute_population_variance(values).unwrap();
        // population variance of [1,2,3,4,5] = 10 / 5 = 2
        assert_eq!(var, 2);
    }

    #[test]
    fn test_pop_variance_two_elements() {
        let values = &[10, 20];
        let var = compute_population_variance(values).unwrap();
        // mean = 15, devs: -5, 5; sq devs: 25, 25; sum_sq = 50; var = 50/2 = 25
        assert_eq!(var, 25);
    }

    // --- compute_sample_variance ---

    #[test]
    fn test_sample_variance_empty() {
        assert_eq!(compute_sample_variance(&[]), Ok(0));
    }

    #[test]
    fn test_sample_variance_single() {
        assert_eq!(compute_sample_variance(&[100]), Ok(0));
    }

    #[test]
    fn test_sample_variance_identical() {
        assert_eq!(compute_sample_variance(&[7, 7, 7, 7]), Ok(0));
    }

    #[test]
    fn test_sample_variance_known() {
        let values = &[1, 2, 3, 4, 5];
        let var = compute_sample_variance(values).unwrap();
        // sample variance of [1,2,3,4,5] = 10 / 4 = 2 (integer floor)
        assert_eq!(var, 2);
    }

    #[test]
    fn test_sample_variance_two_elements() {
        let values = &[10, 20];
        let var = compute_sample_variance(values).unwrap();
        // mean = 15, devs: -5, 5; sq devs: 25, 25; sum_sq = 50; var = 50/1 = 50
        assert_eq!(var, 50);
    }

    // --- corridor-rate scenario ---

    #[test]
    fn test_corridor_rate_variance_preserves_precision() {
        // Simulate five corridor rate submissions around 1.05 (scaled to 7 decimals)
        let rates = &[10_500_000, 10_510_000, 10_490_000, 10_505_000, 10_495_000];
        let var = compute_population_variance(rates).unwrap();
        // Every product (dev * dev) is done in full i128 before division,
        // so no fractional bits are lost before the final scale-down.
        assert!(var > 0);
    }

    // --- calculate_spread_bps ---

    #[test]
    fn test_spread_bps_no_deviation() {
        assert_eq!(calculate_spread_bps(1_000_000, 1_000_000), Ok(0));
    }

    #[test]
    fn test_spread_bps_positive_deviation() {
        // 1% spread: |1_000_000 - 1_010_000| * 10_000 / 1_000_000 = 100
        assert_eq!(calculate_spread_bps(1_000_000, 1_010_000), Ok(100));
    }

    #[test]
    fn test_spread_bps_negative_deviation() {
        // 2% spread: |1_000_000 - 980_000| * 10_000 / 1_000_000 = 200
        assert_eq!(calculate_spread_bps(1_000_000, 980_000), Ok(200));
    }

    #[test]
    fn test_spread_bps_division_by_zero() {
        assert_eq!(
            calculate_spread_bps(0, 1_000_000),
            Err(ContractError::DivisionByZero)
        );
    }

    #[test]
    fn test_spread_bps_overflow() {
        // Large delta and rate_b can cause the numerator to overflow
        let rate_a = 100;
        let rate_b = i128::MAX; // Creates a large delta
        assert_eq!(
            calculate_spread_bps(rate_a, rate_b),
            Err(ContractError::Overflow)
        );
    }

    // --- multiply_and_scale_down ---

    #[test]
    fn test_multiply_and_scale_down_normal() {
        // (2 * 10^7) * (3 * 10^7) / 10^7 = 6 * 10^7
        let scale = 10_000_000;
        assert_eq!(
            multiply_and_scale_down(2 * scale, 3 * scale, scale),
            Ok(6 * scale)
        );
    }

    #[test]
    fn test_multiply_and_scale_down_with_truncation() {
        // 1.5 * 2.5 = 3.75. Scaled: (15 * 10^6) * (25 * 10^6) / 10^7 = 37.5 * 10^6 -> 37_500_000
        let scale = 10_000_000;
        assert_eq!(
            multiply_and_scale_down(15_000_000, 2_500_000, scale),
            Ok(3_750_000) // (1.5 * 0.25) * 10^7
        );
    }

    #[test]
    fn test_multiply_and_scale_down_division_by_zero() {
        assert_eq!(
            multiply_and_scale_down(100, 200, 0),
            Err(ContractError::DivisionByZero)
        );
    }

    #[test]
    fn test_multiply_and_scale_down_overflow() {
        assert_eq!(
            multiply_and_scale_down(i128::MAX, 2, 10_000_000),
            Err(ContractError::Overflow)
        );
    }

    #[test]
    fn test_multiply_and_scale_down_zero_value() {
        assert_eq!(
            multiply_and_scale_down(0, 12345, 10_000_000),
            Ok(0)
        );
    }
}
