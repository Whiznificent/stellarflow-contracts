use soroban_sdk::{Env, String};

use crate::Error;

/// Format a scaled integer price into a human-readable decimal string.
///
/// Inserts a decimal point at the position indicated by `decimals`.
/// Works entirely with byte arrays — no `format!`, no `std`, no heap allocations
/// beyond the final Soroban `String`.
///
/// # Examples
/// ```text
/// format_price(env, 75050, 2)  => "750.50"
/// format_price(env, 50,    3)  => "0.050"
/// format_price(env, 1,     0)  => "1"
/// format_price(env, 0,     2)  => "0.00"
/// ```

// pub fn format_price(env: &Env, price: i128, decimals: u32) -> String {
//     // --- 1. Convert the absolute value to ASCII digits in a fixed buffer ------
//     // i128::MAX is 39 digits; 1 sign + 39 digits + 1 dot + 1 NUL = 42 bytes is safe.
//     const BUF: usize = 42;
//     let mut digits = [0u8; BUF]; // ASCII digit buffer (filled right-to-left)
//     let mut len = 0usize;
//     let negative = price < 0;
//     // Use u128 so we can safely negate i128::MIN without overflow.
//     let mut remaining: u128 = if negative {
//         (price as i128).unsigned_abs()
//     } else {
//         price as u128
//     };

//     // Edge case: price == 0
//     if remaining == 0 {
//         digits[BUF - 1] = b'0';
//         len = 1;
//     } else {
//         while remaining > 0 {
//             len += 1;
//             digits[BUF - len] = b'0' + (remaining % 10) as u8;
//             remaining /= 10;
//         }
//     }
//     // digits[BUF-len .. BUF] now holds the ASCII digits, most-significant first.

//     // --- 2. Build the output byte slice into a second fixed buffer ------------
//     // Max output length: 1 (sign) + 39 (digits) + 1 (dot) = 41 bytes.
//     let mut out = [0u8; 41];
//     let mut pos = 0usize;

//     let decimals = decimals as usize;

//     if negative {
//         out[pos] = b'-';
//         pos += 1;
//     }

//     if decimals == 0 {
//         // No decimal point needed — copy digits straight through.
//         let src = &digits[BUF - len..BUF];
//         out[pos..pos + len].copy_from_slice(src);
//         pos += len;
//     } else if len <= decimals {
//         // The integer part is zero; we need leading "0." and possibly leading
//         // fractional zeros.  e.g. price=50, decimals=3 → "0.050"
//         out[pos] = b'0';
//         pos += 1;
//         out[pos] = b'.';
//         pos += 1;

//         // Pad with zeros until we reach the actual digits.
//         let leading_zeros = decimals - len;
//         for _ in 0..leading_zeros {
//             out[pos] = b'0';
//             pos += 1;
//         }

//         let src = &digits[BUF - len..BUF];
//         out[pos..pos + len].copy_from_slice(src);
//         pos += len;
//     } else {
//         // Normal case: integer part has (len - decimals) digits.
//         let int_len = len - decimals;
//         let src = &digits[BUF - len..BUF];

//         out[pos..pos + int_len].copy_from_slice(&src[..int_len]);
//         pos += int_len;

//         out[pos] = b'.';
//         pos += 1;

//         out[pos..pos + decimals].copy_from_slice(&src[int_len..]);
//         pos += decimals;
//     }

//     // --- 3. Wrap in a Soroban String ------------------------------------------
//     // `from_bytes` expects a byte slice, not a soroban_sdk::Bytes.
//     String::from_bytes(env, &out[..pos])
// }

/// Calculate the absolute deviation between a submitted price and the consensus
/// median, expressed in basis points (bps).
///
/// Formula: `|submitted - consensus| * 10_000 / consensus`
///
/// Both values must already be normalized to the same decimal precision before
/// calling. Use [`normalize_to_nine`] if the inputs have different native precisions.
///
/// # Errors
/// - `Error::DeviationConsensusZero` — when `consensus` is zero (divide-by-zero guard).
/// - `Error::PriceMathOverflow` — on arithmetic overflow.
///
/// # Examples
/// ```text
/// calculate_deviation_bps(10_100, 10_000) => Ok(100)   // 1 % = 100 bps
/// calculate_deviation_bps(10_000, 10_000) => Ok(0)     // identical prices
/// calculate_deviation_bps(500, 0)         => Err(DeviationConsensusZero)
/// ```
pub fn calculate_deviation_bps(submitted: i128, consensus: i128) -> Result<u32, Error> {
    if consensus == 0 {
        return Err(Error::DeviationConsensusZero);
    }
    let diff = if submitted >= consensus {
        submitted - consensus
    } else {
        consensus - submitted
    };
    // diff * 10_000 / consensus — use saturating mul so extreme submissions
    // (e.g. i128::MAX) don't panic; they saturate to u32::MAX which maps to
    // the highest DeviationTier (Manipulation).
    let bps = match diff.checked_mul(10_000) {
        Some(v) => v.checked_div(consensus).ok_or(Error::PriceMathOverflow)?,
        None => i128::MAX,
    };
    Ok(bps.min(u32::MAX as i128) as u32)
}

pub fn normalize_to_seven(value: i128, input_decimals: u32) -> Result<i128, Error> {
    // Early trap: validate input value is within safe range
    if value == i128::MIN || value == i128::MAX {
        return Err(Error::PriceMathOverflow);
    }

    if input_decimals < 7 {
        let diff = 7 - input_decimals;
        let multiplier = 10_i128
            .checked_pow(diff)
            .ok_or(Error::PriceMathOverflow)?;
        
        // Explicit overflow trap before multiplication
        value
            .checked_mul(multiplier)
            .ok_or(Error::PriceMathOverflow)
    } else if input_decimals > 7 {
        let diff = input_decimals - 7;
        let divisor = 10_i128
            .checked_pow(diff)
            .ok_or(Error::PriceMathOverflow)?;
        
        // Explicit divide-by-zero trap (though 10^n cannot be zero)
        if divisor == 0 {
            return Err(Error::PriceMathOverflow);
        }
        
        value
            .checked_div(divisor)
            .ok_or(Error::PriceMathOverflow)
    } else {
        Ok(value)
    }
}

/// Normalize a raw price to 9 fixed-point decimals regardless of the asset's
/// native decimal precision.
///
/// All internal math uses 9-decimal fixed-point so that developers never need
/// to write different logic for different assets.
///
/// Formula: `price * 10^(9 - native_decimals)`
///
/// This function uses checked arithmetic throughout to prevent integer truncation
/// during multi-hop liquidity path calculations.
///
/// # Examples
/// ```text
/// normalize_to_nine(1_000_000_0, 7)  => 1_000_000_000  (XLM, 7 dec → 9 dec)
/// normalize_to_nine(100,         2)  => 10_000_000_000  (NGN, 2 dec → 9 dec)
/// normalize_to_nine(1_000_000_000, 9) => 1_000_000_000  (already 9 dec, no-op)
/// normalize_to_nine(1_000_000_000_00, 11) => 1_000_000_000 (scale down)
/// ```
pub fn normalize_to_nine(value: i128, native_decimals: u32) -> Result<i128, Error> {
    const TARGET: u32 = 9;
    const INTERIOR_SCALE: i128 = 1_000_000_000_000_000; // 10^15

    // NOTE: INTERIOR_SCALE is chosen so that the final result remains within
    // the project's 9-decimal fixed-point footprint by dividing back down
    // after the translation.

    // Early trap: validate input value is within safe range for scaled arithmetic
    if value == i128::MIN || value == i128::MAX {
        return Err(Error::PriceMathOverflow);
    }

    // Explicit overflow trap on initial scaling operation
    let scaled = value
        .checked_mul(INTERIOR_SCALE)
        .ok_or(Error::PriceMathOverflow)?;

    if native_decimals < TARGET {
        let diff = TARGET - native_decimals;
        
        // Trap power overflow early
        let multiplier = 10_i128
            .checked_pow(diff)
            .ok_or(Error::PriceMathOverflow)?;
        
        // Use checked_mul to explicitly trap multiplication overflow
        scaled
        let multiplier = 10_i128.checked_pow(diff).ok_or(Error::PriceMathOverflow)?;
        scaled
            .checked_mul(multiplier)
            .ok_or(Error::PriceMathOverflow)?
    } else if native_decimals > TARGET {
        let diff = native_decimals - TARGET;
        
        // Trap power overflow early
        let divisor = 10_i128
            .checked_pow(diff)
            .ok_or(Error::PriceMathOverflow)?;
        
        // Explicit divide-by-zero trap (defensive, 10^n cannot be zero)
        if divisor == 0 {
            return Err(Error::PriceMathOverflow);
        }
        
        // Use checked_div to trap any division anomalies
        scaled
            .checked_div(divisor)
            .ok_or(Error::PriceMathOverflow)?
    } else {
        scaled
    };

    // Final checked division to scale back down
    normalized_in_interior_space
        .checked_div(INTERIOR_SCALE)
        .ok_or(Error::PriceMathOverflow)
        let divisor = 10_i128.checked_pow(diff).ok_or(Error::PriceMathOverflow)?;
        require_nonzero_denominator(divisor)?;
        scaled
            .checked_div(divisor)
            .ok_or(Error::PriceMathOverflow)?
    } else {
        scaled
    };

    require_nonzero_denominator(INTERIOR_SCALE)?;
    normalized_in_interior_space
        .checked_div(INTERIOR_SCALE)
        .ok_or(Error::PriceMathOverflow)
}

/// Calculate the inverse of a price (e.g., NGN/XLM → XLM/NGN).
///
/// Uses a fixed-point scale factor of `10^decimals` so that the result
/// preserves the same decimal precision as the input.
///
/// Formula: `(10^decimals * 10^decimals) / price`
///
/// This function uses Soroban's native checked arithmetic to explicitly trap
/// overflow errors during multi-hop regional asset calculations.
///
/// # Returns
/// `Some(inverse)` on success, or `None` when `price` is zero (divide-by-zero)
/// or when overflow occurs.
///
/// # Examples
/// ```text
/// calculate_inverse_price(2_000, 3)  => Some(500_000)   // 1/2.000 = 0.500 (scaled)
/// calculate_inverse_price(0,     7)  => None             // divide-by-zero guard
/// ```
pub fn calculate_inverse_price(price: i128, decimals: u32) -> Option<i128> {
    // Explicit early trap: zero price guard
    if price == 0 {
        return None;
    }
    
    // Explicit early trap: extreme value guard
    if price == i128::MIN || price == i128::MAX {
        return None;
    }
    
    // Trap power overflow explicitly
    let scale = 10_i128.checked_pow(decimals)?;
    
    // Trap multiplication overflow explicitly
    let numerator = scale.checked_mul(scale)?;
    
    // Trap division overflow/error explicitly
    numerator.checked_div(price)
}

/// Require that a denominator is non-zero before performing division.
///
/// Returns `Ok(())` when `n != 0`, or `Err(Error::InvalidDenominator)` when `n` is zero.
/// Call this proactively before every division to prevent runtime panics
/// and to provide a clear error signal to callers.
pub fn require_nonzero_denominator(n: i128) -> Result<(), Error> {
    if n == 0 {
        Err(Error::InvalidDenominator)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    // --- format_price tests ---------------------------------------------------
    // NOTE: commented out because format_price itself is commented out pending
    // a decision on whether to re-enable the formatted string output feature.

    // #[test]
    // fn test_format_price_normal() {
    //     let env = Env::default();
    //     // 75050 with 2 decimals → "750.50"
    //     let s = format_price(&env, 75050, 2);
    //     assert_eq!(s.to_string(), "750.50");
    // }

    // #[test]
    // fn test_format_price_small_value() {
    //     let env = Env::default();
    //     // 50 with 3 decimals → "0.050"
    //     let s = format_price(&env, 50, 3);
    //     assert_eq!(s.to_string(), "0.050");
    // }

    // #[test]
    // fn test_format_price_no_decimals() {
    //     let env = Env::default();
    //     // 12345 with 0 decimals → "12345"
    //     let s = format_price(&env, 12345, 0);
    //     assert_eq!(s.to_string(), "12345");
    // }

    // #[test]
    // fn test_format_price_zero() {
    //     let env = Env::default();
    //     // 0 with 2 decimals → "0.00"
    //     let s = format_price(&env, 0, 2);
    //     assert_eq!(s.to_string(), "0.00");
    // }

    // #[test]
    // fn test_format_price_exact_decimal_boundary() {
    //     let env = Env::default();
    //     // 1 with 1 decimal → "0.1"
    //     let s = format_price(&env, 1, 1);
    //     assert_eq!(s.to_string(), "0.1");
    // }

    // #[test]
    // fn test_format_price_negative() {
    //     let env = Env::default();
    //     // -75050 with 2 decimals → "-750.50"
    //     let s = format_price(&env, -75050, 2);
    //     assert_eq!(s.to_string(), "-750.50");
    // }

    // --- calculate_deviation_bps tests ----------------------------------------

    #[test]
    fn test_deviation_bps_identical() {
        assert_eq!(calculate_deviation_bps(10_000, 10_000), Ok(0));
    }

    #[test]
    fn test_deviation_bps_above_consensus() {
        // 10_100 vs 10_000 → 100 bps (1 %)
        assert_eq!(calculate_deviation_bps(10_100, 10_000), Ok(100));
    }

    #[test]
    fn test_deviation_bps_below_consensus() {
        // 9_800 vs 10_000 → 200 bps (2 %)
        assert_eq!(calculate_deviation_bps(9_800, 10_000), Ok(200));
    }

    #[test]
    fn test_deviation_bps_zero_consensus() {
        assert_eq!(
            calculate_deviation_bps(500, 0),
            Err(Error::DeviationConsensusZero)
        );
    }

    #[test]
    fn test_deviation_bps_extreme_saturates_to_u32_max() {
        let result = calculate_deviation_bps(i128::MAX, 1);
        assert_eq!(result, Ok(u32::MAX));
    }

    // --- normalize_to_seven tests ---------------------------------------------

    #[test]
    fn test_normalize_to_seven_scale_up() {
        assert_eq!(normalize_to_seven(150, 2), Ok(15_000_000));
    }

    #[test]
    fn test_normalize_to_seven_scale_down() {
        assert_eq!(normalize_to_seven(100_000_000, 9), Ok(1_000_000));
    }

    #[test]
    fn test_normalize_to_seven_no_scale() {
        assert_eq!(normalize_to_seven(1234567, 7), Ok(1234567));
    }

    // --- normalize_to_nine tests ---------------------------------------------

    #[test]
    fn test_normalize_to_nine_scale_up_from_7() {
        // XLM has 7 decimals: multiply by 10^2
        assert_eq!(normalize_to_nine(10_000_000, 7), Ok(1_000_000_000));
    }

    #[test]
    fn test_normalize_to_nine_scale_up_from_2() {
        // NGN has 2 decimals: multiply by 10^7
        assert_eq!(normalize_to_nine(100, 2), Ok(10_000_000_000));
    }

    #[test]
    fn test_normalize_to_nine_no_scale() {
        // Already 9 decimals — no-op
        assert_eq!(normalize_to_nine(1_000_000_000, 9), Ok(1_000_000_000));
    }

    #[test]
    fn test_normalize_to_nine_scale_down() {
        // 11 decimals → divide by 10^2
        assert_eq!(normalize_to_nine(100_000_000_000, 11), Ok(1_000_000_000));
    }

    #[test]
    fn test_normalize_to_nine_zero_decimals() {
        // 0 native decimals → multiply by 10^9
        assert_eq!(normalize_to_nine(1, 0), Ok(1_000_000_000));
    }

    // --- Overflow protection tests for multi-hop calculations -----------------

    #[test]
    fn test_normalize_to_nine_extreme_value_rejection() {
        // Extreme values should be trapped early to prevent overflow
        assert_eq!(
            normalize_to_nine(i128::MAX, 0),
            Err(Error::PriceMathOverflow)
        );
        assert_eq!(
            normalize_to_nine(i128::MIN, 0),
            Err(Error::PriceMathOverflow)
        );
    }

    #[test]
    fn test_normalize_to_nine_large_safe_value() {
        // Large but safe values should still work
        let large_value = 1_000_000_000_000_000_i128; // 10^15
        let result = normalize_to_nine(large_value, 9);
        assert!(result.is_ok());
    }

    #[test]
    fn test_normalize_to_seven_extreme_value_rejection() {
        // Extreme values should be trapped early
        assert_eq!(
            normalize_to_seven(i128::MAX, 0),
            Err(Error::PriceMathOverflow)
        );
        assert_eq!(
            normalize_to_seven(i128::MIN, 0),
            Err(Error::PriceMathOverflow)
        );
    }

    #[test]
    fn test_calculate_inverse_price_extreme_values() {
        // Extreme values should return None to prevent overflow
        assert_eq!(calculate_inverse_price(i128::MAX, 9), None);
        assert_eq!(calculate_inverse_price(i128::MIN, 9), None);
    }

    #[test]
    fn test_calculate_inverse_price_safe_values() {
        // Normal operation with safe values
        assert_eq!(calculate_inverse_price(2_000, 3), Some(500_000));
        assert_eq!(calculate_inverse_price(1_000_000_000, 9), Some(1_000_000_000));
    }

    #[test]
    fn test_multi_hop_simulation_no_overflow() {
        // Simulate a multi-hop path: Asset A -> B -> C
        // Each hop normalizes and calculates, ensuring no overflow
        let asset_a_price = 1_000_000_000; // 9 decimals
        let asset_b_price = 2_000_000_000; // 9 decimals
        
        // First hop: A to B
        let hop1 = normalize_to_nine(asset_a_price, 9);
        assert!(hop1.is_ok());
        
        // Second hop: B to C (via inverse)
        let inverse_b = calculate_inverse_price(asset_b_price, 9);
        assert!(inverse_b.is_some());
        
        // The chain should complete without overflow
        assert_eq!(hop1.unwrap(), 1_000_000_000);
        assert_eq!(inverse_b.unwrap(), 500_000);
    }
}

