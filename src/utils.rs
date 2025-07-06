// utils.rs

use primitive_types::U256;
use std::f64::consts::E;

/// Simulate V2 swap with fee and direction (PancakeSwap V2 style)
/// Uses exact PancakeSwap V2 formula: amountOut = (amountIn * 9975 * reserveOut) / (reserveIn * 10000 + amountIn * 9975)
pub fn simulate_v2_swap_safe(
    amount_in: f64,
    reserve_in: U256,
    reserve_out: U256,
    fee_bps: u32,         // fee in basis points (e.g., 25 for 0.25% PancakeSwap V2)
    is_forward: bool,
) -> f64 {
    // Add overflow protection for as_u128() calls
    let r_in = if reserve_in > U256::from(u128::MAX) { 
        u128::MAX as f64 
    } else { 
        reserve_in.as_u128() as f64 
    };
    let r_out = if reserve_out > U256::from(u128::MAX) { 
        u128::MAX as f64 
    } else { 
        reserve_out.as_u128() as f64 
    };
    
    // PancakeSwap V2 uses 0.25% fee = 9975/10000
    let fee_numerator = 10000.0 - fee_bps as f64;
    let fee_denominator = 10000.0;

    if r_in == 0.0 || r_out == 0.0 || amount_in == 0.0 {
        return 0.0;
    }

    let amount_in_with_fee = amount_in * fee_numerator / fee_denominator;
    let numerator = amount_in_with_fee * r_out;
    let denominator = r_in * fee_denominator + amount_in_with_fee;
    let output = numerator / denominator;

    if output <= 0.0 || output.is_nan() || output.is_infinite() {
        return 0.0;
    }

    output
}

/// Simulate V2 reverse swap (getAmountsIn) - calculates input needed for desired output
/// Uses exact PancakeSwap V2 formula: amountIn = (reserveIn * amountOut * 10000) / ((reserveOut - amountOut) * 9975) + 1
pub fn simulate_v2_swap_reverse_safe(
    amount_out: f64,
    reserve_in: U256,
    reserve_out: U256,
    fee_bps: u32,         // fee in basis points (e.g., 25 for 0.25% PancakeSwap V2)
    is_forward: bool,
) -> f64 {
    // Add overflow protection for as_u128() calls
    let r_in = if reserve_in > U256::from(u128::MAX) { 
        u128::MAX as f64 
    } else { 
        reserve_in.as_u128() as f64 
    };
    let r_out = if reserve_out > U256::from(u128::MAX) { 
        u128::MAX as f64 
    } else { 
        reserve_out.as_u128() as f64 
    };
    
    // PancakeSwap V2 uses 0.25% fee = 9975/10000
    let fee_numerator = 10000.0 - fee_bps as f64;
    let fee_denominator = 10000.0;

    if r_in == 0.0 || r_out == 0.0 || amount_out == 0.0 || amount_out >= r_out {
        return 0.0;
    }

    let numerator = r_in * amount_out * fee_denominator;
    let denominator = (r_out - amount_out) * fee_numerator;
    let amount_in = numerator / denominator + 1.0; // +1 for rounding up

    if amount_in <= 0.0 || amount_in.is_nan() || amount_in.is_infinite() {
        return 0.0;
    }

    amount_in
}

/// Simulate V3 swap using proper Uniswap V3 math
/// This function now uses the correct V3 formulas from v3_math.rs
pub fn simulate_v3_swap_precise(
    amount_in: f64,
    sqrt_price_x96: U256,
    liquidity: U256,
    fee_bps: u32,
    is_forward: bool,
) -> f64 {
    use crate::v3_math::simulate_v3_swap;
    
    // Convert is_forward to zero_for_one (V3 terminology)
    // is_forward = true means token0->token1, which is zero_for_one = true
    let zero_for_one = is_forward;
    
    simulate_v3_swap(
        U256::from(amount_in as u128),
        sqrt_price_x96,
        liquidity,
        fee_bps,
        zero_for_one,
    ).map(|result| {
        // Add overflow protection for as_u128() call
        if result > U256::from(u128::MAX) {
            u128::MAX as f64
        } else {
            result.as_u128() as f64
        }
    }).unwrap_or(0.0)
}
