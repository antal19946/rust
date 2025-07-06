use ethers::types::U256;
use primitive_types::U256 as PrimitiveU256;

pub const Q96: u128 = 2u128.pow(96);

/// Computes a * b / denominator, returns None on overflow or div by zero
#[inline]
pub fn mul_div(a: U256, b: U256, denominator: U256) -> Option<U256> {
    a.checked_mul(b)?.checked_div(denominator)
}

/// Convert sqrtPriceX96 to actual price with overflow protection
#[inline]
pub fn sqrt_price_x96_to_price(sqrt_price_x96: U256) -> f64 {
    // Handle extremely large values to prevent overflow
    let sqrt_price_u128 = if sqrt_price_x96 > U256::from(u128::MAX) {
        u128::MAX
    } else {
        sqrt_price_x96.as_u128()
    };
    
    let sqrt_price = sqrt_price_u128 as f64;
    let price = (sqrt_price / Q96 as f64).powi(2);
    
    // Clamp to reasonable range to prevent infinite values
    if price.is_infinite() || price.is_nan() || price > 1e20 {
        1e20
    } else if price < 1e-20 {
        1e-20
    } else {
        price
    }
}

/// Convert price to sqrtPriceX96 with overflow protection
#[inline]
pub fn price_to_sqrt_price_x96(price: f64) -> U256 {
    let sqrt_price = (price.sqrt() * Q96 as f64) as u128;
    U256::from(sqrt_price)
}

/// Correct Uniswap V3 swap simulation using proper V3 formulas
/// Based on Uniswap V3 whitepaper and official implementation
pub fn simulate_v3_swap(
    amount_in: U256,
    sqrt_price_x96: U256,
    liquidity: U256,
    fee_bps: u32,
    zero_for_one: bool,
) -> Option<U256> {
    if liquidity.is_zero() || sqrt_price_x96.is_zero() {
        return None;
    }

    // Sanity check: reasonable values
    if sqrt_price_x96 > U256::from(u128::MAX) || liquidity > U256::from(u128::MAX) {
        return None;
    }

    // Apply fee (e.g., 3000 bps = 0.3% = 997/1000)
    let fee_numerator = 1000000u32 - fee_bps; // 1000000 - 3000 = 997000 (99.7%)
    let fee_denominator = 1000000u32;
    
    let amount_in_with_fee = amount_in.checked_mul(U256::from(fee_numerator))?.checked_div(U256::from(fee_denominator))?;

    if zero_for_one {
        // Token0 -> Token1: price DECREASES (token0 becomes cheaper)
        // Formula: sqrtP_new = (L * Q96 * sqrtP_cur) / (L * Q96 + netIn_0 * sqrtP_cur)
        let numerator = liquidity.checked_mul(U256::from(Q96))?.checked_mul(sqrt_price_x96)?;
        let denominator = liquidity.checked_mul(U256::from(Q96))?.checked_add(amount_in_with_fee.checked_mul(sqrt_price_x96)?)?;
        
        if denominator <= U256::zero() {
            return None; // Avoid division by zero
        }
        
        let sqrt_price_new = numerator.checked_div(denominator)?;
        
        // Amount1 out = L * (sqrtP_cur - sqrtP_new) / Q96
        let delta_sqrt = sqrt_price_x96.checked_sub(sqrt_price_new)?;
        let amount_out = liquidity.checked_mul(delta_sqrt)?.checked_div(U256::from(Q96))?;
        
        // Sanity check: amount out should be reasonable
        if amount_out > amount_in.checked_mul(U256::from(1000u32))? {
            return None; // More than 1000x output is unrealistic
        }
        
        Some(amount_out)
    } else {
        // Token1 -> Token0: price INCREASES (token1 becomes cheaper)
        // Formula: sqrtP_new = sqrtP_cur + (netIn_1 * Q96) / L
        let add = amount_in_with_fee.checked_mul(U256::from(Q96))?.checked_div(liquidity)?;
        let sqrt_price_new = sqrt_price_x96.checked_add(add)?;
        
        // Amount0 out = L * (1/sqrtP_cur - 1/sqrtP_new)
        // Convert to: (L * (sqrtP_new - sqrtP_cur)) / (sqrtP_new * sqrtP_cur / Q96)
        let delta_sqrt = sqrt_price_new.checked_sub(sqrt_price_x96)?;
        
        // Compute output via fraction: (L * delta_sqrt * Q96) / (sqrt_price_new * sqrt_price_current)
        let numerator = liquidity.checked_mul(delta_sqrt)?.checked_mul(U256::from(Q96))?;
        let denominator = sqrt_price_new.checked_mul(sqrt_price_x96)?.checked_div(U256::from(Q96))?;
        
        if denominator <= U256::zero() {
            return None; // Avoid division by zero
        }
        
        let amount_out = numerator.checked_div(denominator)?;
        
        // Sanity check: amount out should be reasonable
        if amount_out > amount_in.checked_mul(U256::from(1000u32))? {
            return None; // More than 1000x output is unrealistic
        }
        
        Some(amount_out)
    }
}

/// Calculate V3 buy amount needed for a given output (reverse calculation)
pub fn calculate_v3_buy_amount(
    amount_out: U256,
    sqrt_price_x96: U256,
    liquidity: U256,
    fee_bps: u32,
    zero_for_one: bool,
) -> Option<U256> {
    if liquidity.is_zero() || sqrt_price_x96.is_zero() {
        return None;
    }

    // Sanity check: reasonable values
    if sqrt_price_x96 > U256::from(u128::MAX) || liquidity > U256::from(u128::MAX) {
        return None;
    }

    // Sanity check: amount out should be reasonable
    if amount_out > liquidity {
        return None; // Can't output more than liquidity
    }

    let fee_numerator = 1000000u32 - fee_bps; // 1000000 - 3000 = 997000 (99.7%)
    let fee_denominator = 1000000u32;

    if zero_for_one {
        // We want token1, need to calculate token0 input
        // Reverse of token0->token1 formula
        // amount1Out = L * (sqrtP_cur - sqrtP_new) / Q96
        // So: sqrtP_new = sqrtP_cur - (amount1Out * Q96) / L
        let delta_sqrt = amount_out.checked_mul(U256::from(Q96))?.checked_div(liquidity)?;
        
        if delta_sqrt >= sqrt_price_x96 {
            return None; // Can't reduce price below zero
        }
        
        let sqrt_price_new = sqrt_price_x96.checked_sub(delta_sqrt)?;
        
        // Now reverse the sqrt price formula to get input
        // sqrtP_new = (L * Q96 * sqrtP_cur) / (L * Q96 + netIn_0 * sqrtP_cur)
        // Rearranging: netIn_0 = (L * Q96 * sqrtP_cur - L * Q96 * sqrtP_new) / (sqrtP_new * sqrtP_cur)
        let numerator = liquidity.checked_mul(U256::from(Q96))?.checked_mul(sqrt_price_x96)?
            .checked_sub(liquidity.checked_mul(U256::from(Q96))?.checked_mul(sqrt_price_new)?)?;
        let denominator = sqrt_price_new.checked_mul(sqrt_price_x96)?;
        
        if denominator <= U256::zero() {
            return None;
        }
        
        let amount_in_with_fee = numerator.checked_div(denominator)?;
        let amount_in = amount_in_with_fee.checked_mul(U256::from(fee_denominator))?.checked_div(U256::from(fee_numerator))?;
        
        // Round up to ensure we get at least the desired output
        let amount_in_rounded = amount_in + U256::one();
        
        // Sanity check: input should be reasonable
        if amount_in_rounded > amount_out.checked_mul(U256::from(1000u32))? {
            return None; // More than 1000x input is unrealistic
        }
        
        Some(amount_in_rounded)
    } else {
        // We want token0, need to calculate token1 input
        // IMPROVED: Use exact formula instead of approximation
        // amount0Out = L * (1/sqrtP_cur - 1/sqrtP_new)
        // Rearranging: sqrtP_new = L * sqrtP_cur / (L - amount0Out * sqrtP_cur)
        
        // Calculate the exact sqrt price needed
        let numerator = liquidity.checked_mul(sqrt_price_x96)?;
        let denominator = liquidity.checked_sub(amount_out.checked_mul(sqrt_price_x96)?.checked_div(U256::from(Q96))?)?;
        
        if denominator <= U256::zero() {
            return None; // Can't output this much token0
        }
        
        let sqrt_price_new = numerator.checked_div(denominator)?;
        
        // Now calculate the token1 input needed for this price change
        // sqrtP_new = sqrtP_cur + (netIn_1 * Q96) / L
        // So: netIn_1 = (sqrtP_new - sqrtP_cur) * L / Q96
        let delta_sqrt = sqrt_price_new.checked_sub(sqrt_price_x96)?;
        let amount_in_with_fee = delta_sqrt.checked_mul(liquidity)?.checked_div(U256::from(Q96))?;
        let amount_in = amount_in_with_fee.checked_mul(U256::from(fee_denominator))?.checked_div(U256::from(fee_numerator))?;
        
        // Round up to ensure we get at least the desired output
        let amount_in_rounded = amount_in + U256::one();
        
        // Sanity check: input should be reasonable
        if amount_in_rounded > amount_out.checked_mul(U256::from(1000u32))? {
            return None; // More than 1000x input is unrealistic
        }
        
        Some(amount_in_rounded)
    }
}

/// Get next sqrt price from input amount (correct V3 formula)
#[inline]
pub fn get_next_sqrt_price_from_input(
    sqrt_price_x96: U256,
    liquidity: U256,
    amount_in: U256,
    zero_for_one: bool,
) -> Option<U256> {
    if liquidity.is_zero() {
        return None;
    }

    if zero_for_one {
        // Token0 -> Token1: price decreases
        // Formula: sqrtP_new = (L * Q96 * sqrtP_cur) / (L * Q96 + netIn_0 * sqrtP_cur)
        let numerator = liquidity.checked_mul(U256::from(Q96))?.checked_mul(sqrt_price_x96)?;
        let denominator = liquidity.checked_mul(U256::from(Q96))?.checked_add(amount_in.checked_mul(sqrt_price_x96)?)?;
        
        if denominator <= U256::zero() {
            return None;
        }
        
        numerator.checked_div(denominator)
    } else {
        // Token1 -> Token0: price increases
        // Formula: sqrtP_new = sqrtP_cur + (netIn_1 * Q96) / L
        let add = amount_in.checked_mul(U256::from(Q96))?.checked_div(liquidity)?;
        sqrt_price_x96.checked_add(add)
    }
}

/// Get next sqrt price from output amount (correct V3 formula)
#[inline]
pub fn get_next_sqrt_price_from_output(
    sqrt_price_x96: U256,
    liquidity: U256,
    amount_out: U256,
    zero_for_one: bool,
) -> Option<U256> {
    if liquidity.is_zero() {
        return None;
    }

    if zero_for_one {
        // Token0 -> Token1: we want token1 out, so price decreases
        // amount1Out = L * (sqrtP_cur - sqrtP_new) / Q96
        // So: sqrtP_new = sqrtP_cur - (amount1Out * Q96) / L
        let delta_sqrt = amount_out.checked_mul(U256::from(Q96))?.checked_div(liquidity)?;
        
        if delta_sqrt >= sqrt_price_x96 {
            return None; // Can't reduce price below zero
        }
        
        sqrt_price_x96.checked_sub(delta_sqrt)
    } else {
        // Token1 -> Token0: we want token0 out, so price increases
        // amount0Out = L * (1/sqrtP_cur - 1/sqrtP_new)
        // This is complex to solve for sqrtP_new, so we'll use approximation
        // For small amounts: sqrtP_new ≈ sqrtP_cur + (amount0Out * sqrtP_cur^2) / (L * Q96)
        let delta_sqrt = amount_out.checked_mul(sqrt_price_x96)?.checked_mul(sqrt_price_x96)?
            .checked_div(liquidity.checked_mul(U256::from(Q96))?)?;
        
        sqrt_price_x96.checked_add(delta_sqrt)
    }
}

/// Test V3 math functions with realistic values
pub fn test_v3_math() {
    println!("🧪 Testing V3 Math Functions (Correct Uniswap V3)...");
    
    // Test with reasonable values (1:1 price)
    let sqrt_price_x96 = U256::from(Q96); // 1.0 price
    let liquidity = U256::from(1000000000000000000u128); // 1e18
    let amount_in = U256::from(100000000000000000u128); // 0.1e18
    
    println!("Test values:");
    println!("  sqrtPriceX96: {}", sqrt_price_x96);
    println!("  liquidity: {}", liquidity);
    println!("  amount_in: {}", amount_in);
    
    // Test sell simulation (token0 -> token1)
    if let Some(amount_out) = simulate_v3_swap(amount_in, sqrt_price_x96, liquidity, 3000, true) {
        println!("✅ V3 sell simulation (token0->token1): {} -> {}", amount_in, amount_out);
        
        // Calculate profit percentage
        let profit = if amount_out > amount_in {
            amount_out - amount_in
        } else {
            U256::zero()
        };
        
        if !profit.is_zero() {
            let profit_percent = (profit.as_u128() as f64 / amount_in.as_u128() as f64) * 100.0;
            println!("  Profit: {} ({}%)", profit, profit_percent);
            
            // Sanity check: profit should be reasonable
            if profit_percent > 100.0 {
                println!("  ⚠️  WARNING: Unrealistic profit percentage!");
            }
        }
    } else {
        println!("❌ V3 sell simulation failed");
    }
    
    // Test buy calculation (reverse)
    let amount_out = U256::from(100000000000000000u128); // 0.1e18
    if let Some(amount_in_needed) = calculate_v3_buy_amount(amount_out, sqrt_price_x96, liquidity, 3000, true) {
        println!("✅ V3 buy calculation (token1->token0): {} needed for {}", amount_in_needed, amount_out);
        
        // Calculate cost percentage
        let cost_percent = (amount_in_needed.as_u128() as f64 / amount_out.as_u128() as f64) * 100.0;
        println!("  Cost: {} ({}%)", amount_in_needed, cost_percent);
        
        // Sanity check: cost should be reasonable
        if cost_percent > 200.0 {
            println!("  ⚠️  WARNING: Unrealistic cost percentage!");
        }
    } else {
        println!("❌ V3 buy calculation failed");
    }
    
    // Test fee calculation specifically
    println!("\n🔍 Testing Fee Calculation:");
    let test_amount = U256::from(1000000000000000000u128); // 1 token
    let fee_bps = 3000; // 0.3%
    
    // Manual fee calculation check
    let fee_numerator = 1000000u32 - fee_bps; // 997000
    let fee_denominator = 1000000u32;
    let amount_with_fee = test_amount.checked_mul(U256::from(fee_numerator)).unwrap()
        .checked_div(U256::from(fee_denominator)).unwrap();
    
    let fee_percentage = ((test_amount - amount_with_fee).as_u128() as f64 / test_amount.as_u128() as f64) * 100.0;
    println!("  Input: {}", test_amount);
    println!("  After {} bps fee: {} ({}% fee applied)", fee_bps, amount_with_fee, fee_percentage);
    
    // Test with the problematic pool values from the log
    println!("\n🔍 Testing with problematic pool values from log:");
    let problematic_sqrt_price = U256::from_dec_str("79338033694141024166214253871").unwrap();
    let problematic_liquidity = U256::from_dec_str("35815244315094858067783").unwrap();
    
    println!("  sqrtPriceX96: {}", problematic_sqrt_price);
    println!("  liquidity: {}", problematic_liquidity);
    
    let price = sqrt_price_x96_to_price(problematic_sqrt_price);
    println!("  Calculated price: {}", price);
    
    // Test with very small amount to avoid overflow
    let small_amount = U256::from(1000000000000000u128); // 1e15
    if let Some(amount_out) = simulate_v3_swap(small_amount, problematic_sqrt_price, problematic_liquidity, 3000, true) {
        println!("✅ Problematic pool simulation: {} -> {}", small_amount, amount_out);
        
        // Calculate profit percentage
        let profit = if amount_out > small_amount {
            amount_out - small_amount
        } else {
            U256::zero()
        };
        
        if !profit.is_zero() {
            let profit_percent = (profit.as_u128() as f64 / small_amount.as_u128() as f64) * 100.0;
            println!("  Profit: {} ({}%)", profit, profit_percent);
            
            // This should catch the unrealistic 5 million percent profit
            if profit_percent > 100.0 {
                println!("  🚨 CRITICAL: Unrealistic profit detected! This indicates a bug in V3 math.");
                return;
            }
        }
    } else {
        println!("❌ Problematic pool simulation failed");
    }
    
    // Test price direction logic
    println!("\n🔍 Testing price direction logic:");
    let test_sqrt_price = U256::from(Q96); // 1.0 price
    let test_liquidity = U256::from(1000000000000000000u128); // 1e18
    let test_amount = U256::from(100000000000000000u128); // 0.1e18
    
    // Token0 -> Token1: price should DECREASE
    if let Some(new_price) = get_next_sqrt_price_from_input(test_sqrt_price, test_liquidity, test_amount, true) {
        let old_price = sqrt_price_x96_to_price(test_sqrt_price);
        let new_price_f64 = sqrt_price_x96_to_price(new_price);
        println!("  Token0->Token1: Price {} -> {} (decreased: {})", old_price, new_price_f64, old_price > new_price_f64);
    }
    
    // Token1 -> Token0: price should INCREASE
    if let Some(new_price) = get_next_sqrt_price_from_input(test_sqrt_price, test_liquidity, test_amount, false) {
        let old_price = sqrt_price_x96_to_price(test_sqrt_price);
        let new_price_f64 = sqrt_price_x96_to_price(new_price);
        println!("  Token1->Token0: Price {} -> {} (increased: {})", old_price, new_price_f64, old_price < new_price_f64);
    }
    
    // Test exact output calculation for token1->token0
    println!("\n🔍 Testing exact output calculation (token1->token0):");
    let desired_token0_out = U256::from(100000000000000000u128); // 0.1 token0
    if let Some(token1_in_needed) = calculate_v3_buy_amount(desired_token0_out, test_sqrt_price, test_liquidity, 3000, false) {
        println!("  To get {} token0, need {} token1", desired_token0_out, token1_in_needed);
        
        // Verify by simulating forward swap
        if let Some(actual_token0_out) = simulate_v3_swap(token1_in_needed, test_sqrt_price, test_liquidity, 3000, false) {
            println!("  Forward simulation: {} token1 -> {} token0", token1_in_needed, actual_token0_out);
            
            if actual_token0_out >= desired_token0_out {
                println!("  ✅ Exact output calculation verified!");
            } else {
                println!("  ⚠️  Output calculation may be slightly off");
            }
        }
    } else {
        println!("  ❌ Exact output calculation failed");
    }
    
    println!("\n✅ V3 math test completed!");
} 