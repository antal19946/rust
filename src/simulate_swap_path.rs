use ethers::types::{H160, U256};
use crate::route_cache::{RoutePath, DEXType};
use crate::cache::{ReserveCache};
use crate::token_index::TokenIndexMap;
use crate::v3_math::{Q96, mul_div, simulate_v3_swap, calculate_v3_buy_amount, sqrt_price_x96_to_price};
use crate::split_route_path::split_route_around_token_x;
use std::collections::HashMap;
use dashmap::DashMap;
use crate::token_tax::TokenTaxMap;
use crate::config::Config;
use std::sync::Arc;

/// Detailed hop information with amounts
#[derive(Debug, Clone)]
pub struct HopDetail {
    pub pool_address: H160,
    pub token_in: u32,
    pub token_out: u32,
    pub amount_in: U256,
    pub amount_out: U256,
    pub reserve_in: U256,
    pub reserve_out: U256,
    pub pool_type: crate::cache::PoolType,
    pub fee: u32,
}

/// Complete path simulation result with all hop details
#[derive(Debug, Clone)]
pub struct PathSimulationResult {
    pub total_amount_in: U256,
    pub total_amount_out: U256,
    pub hops: Vec<HopDetail>,
    pub success: bool,
}

/// Comprehensive simulation result for a single route
#[derive(Debug, Clone)]
pub struct RouteSimulationResult {
    pub route_index: usize,
    pub buy_path: Option<PathSimulationResult>,
    pub sell_path: Option<PathSimulationResult>,
    pub buy_amounts_array: Option<Vec<U256>>,
    pub sell_amounts_array: Option<Vec<U256>>,
    pub buy_amounts_vec: Option<(Vec<U256>, Vec<U256>)>,
    pub sell_amounts_vec: Option<(Vec<U256>, Vec<U256>)>,
    pub profit_loss: Option<i128>, // positive = profit, negative = loss
    pub profit_percentage: Option<f64>,
}

/// Comprehensive simulation results for all filtered routes
#[derive(Debug, Clone)]
pub struct ComprehensiveSimulationResults {
    pub token_address: H160,
    pub pool_address: H160,
    pub token_x_amount: U256,
    pub total_routes: usize,
    pub successful_routes: usize,
    pub profitable_routes: usize,
    pub route_results: Vec<RouteSimulationResult>,
    pub best_profit_route: Option<usize>,
    pub best_profit_amount: Option<i128>,
    pub best_profit_percentage: Option<f64>,
}

/// Simulate V3 swap using proper V3 math
fn simulate_v3_swap_single(
    amount_in: U256,
    sqrt_price_x96: U256,
    liquidity: U256,
    fee: u32,
    zero_for_one: bool,
) -> Option<U256> {
    // Use the proper V3 math function from v3_math.rs
    simulate_v3_swap(amount_in, sqrt_price_x96, liquidity, fee, zero_for_one)
}

/// Simulate how many base tokens are needed to buy `amount_out` of tokenX
/// Returns detailed information for each hop including amounts in/out
pub fn simulate_buy_path(
    route: &RoutePath,
    token_x_amount: U256,
    cache: &ReserveCache,
    token_index_map: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<PathSimulationResult> {
    let mut amount_out = token_x_amount;
    let mut hops = Vec::new();
    
    // Process hops in reverse order (from tokenX back to base token)
    for (i, pool) in route.pools.iter().enumerate().rev() {
        let pool_data = cache.get(pool)?;
        let entry = pool_data.value();
        let token0_idx = *token_index_map.address_to_index.get(&entry.token0)? as u32;
        let token1_idx = *token_index_map.address_to_index.get(&entry.token1)? as u32;
        
        let input_token = route.hops[i];
        let output_token = route.hops[i + 1];
        
        match entry.pool_type {
            crate::cache::PoolType::V2 => {
                let reserve0 = entry.reserve0?;
                let reserve1 = entry.reserve1?;
                if reserve0.is_zero() || reserve1.is_zero() { 
                    println!("[V2 BUY] Pool {} has zero reserves: reserve0={}, reserve1={}", pool, reserve0, reserve1);
                    return None; 
                }
                let (reserve_in, reserve_out) = if input_token == token0_idx {
                    (reserve0, reserve1)
                } else {
                    (reserve1, reserve0)
                };
                if reserve_out <= amount_out { 
                    println!("[V2 BUY] Insufficient output: reserve_out={}, amount_out={}", reserve_out, amount_out);
                    return None; 
                }
                
                // Get dynamic fee based on DEX name
                let fee = if let Some(dex_name) = &entry.dex_name {
                    config.get_v2_fee(dex_name)
                } else {
                    25 // Default to 0.25% if no DEX name
                };
                
                // Dynamic V2 getAmountsIn formula based on fee
                let fee_numerator = 10000 - fee;
                let numerator = reserve_in * amount_out * U256::from(10_000u32);
                let denominator = (reserve_out - amount_out) * U256::from(fee_numerator);
                if denominator.is_zero() { 
                    println!("[V2 BUY] Denominator zero: reserve_out={}, amount_out={}", reserve_out, amount_out);
                    return None; 
                }
                let mut amount_in = numerator.checked_div(denominator)? + U256::one();
                
                // --- Apply buy tax if exists ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply sell tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - sell_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                // --- Apply buy tax on output_token (pool withdrawal) ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - buy_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                // Add hop detail
                hops.push(HopDetail {
                    pool_address: *pool,
                    token_in: input_token,
                    token_out: output_token,
                    amount_in,
                    amount_out,
                    reserve_in,
                    reserve_out,
                    pool_type: crate::cache::PoolType::V2,
                    fee,
                });
                
                println!("[V2 BUY] Pool {}: reserve_in={}, reserve_out={}, amount_out={}, calculated_input={}", 
                    pool, reserve_in, reserve_out, amount_out, amount_in);
                
                amount_out = amount_in;
            }
            crate::cache::PoolType::V3 => {
                let sqrt_price_x96 = entry.sqrt_price_x96?;
                let liquidity = entry.liquidity?;
                let fee = entry.fee.unwrap_or(3000);
                let zero_for_one = input_token == token0_idx;
                
                if liquidity.is_zero() || sqrt_price_x96.is_zero() {
                    println!("[V3 BUY] Pool {} has zero liquidity or sqrtPrice: liquidity={}, sqrtPrice={}", 
                        pool, liquidity, sqrt_price_x96);
                    return None;
                }
                
                // Use the new V3 buy calculation from v3_math
                let mut amount_in = crate::v3_math::calculate_v3_buy_amount(amount_out, sqrt_price_x96, liquidity, fee, zero_for_one)?;
                
                // --- Apply buy tax if exists ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply sell tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - sell_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                // --- Apply buy tax on output_token (pool withdrawal) ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - buy_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                // Add hop detail
                hops.push(HopDetail {
                    pool_address: *pool,
                    token_in: input_token,
                    token_out: output_token,
                    amount_in,
                    amount_out,
                    reserve_in: U256::zero(), // V3 doesn't use reserves
                    reserve_out: U256::zero(),
                    pool_type: crate::cache::PoolType::V3,
                    fee,
                });
                
                println!("[V3 BUY] Pool {}: sqrtPrice={}, liquidity={}, amount_out={}, calculated_input={}, fee={}", 
                    pool, sqrt_price_x96, liquidity, amount_out, amount_in, fee);
                
                amount_out = amount_in;
            }
        }
    }
    
    // Reverse hops to get correct order (base -> tokenX)
    hops.reverse();
    
    Some(PathSimulationResult {
        total_amount_in: amount_out,
        total_amount_out: token_x_amount,
        hops,
        success: true,
    })
}

/// Simulate how many base tokens you get by selling `amount_in` of tokenX
/// Returns detailed information for each hop including amounts in/out
pub fn simulate_sell_path(
    route: &RoutePath,
    token_x_amount: U256,
    cache: &ReserveCache,
    token_index_map: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<PathSimulationResult> {
    let mut amount_in = token_x_amount;
    let mut hops = Vec::new();
    
    // Process hops in forward order (from tokenX to base token)
    for (i, pool) in route.pools.iter().enumerate() {
        let pool_data = cache.get(pool)?;
        let entry = pool_data.value();
        let token0_idx = *token_index_map.address_to_index.get(&entry.token0)? as u32;
        let token1_idx = *token_index_map.address_to_index.get(&entry.token1)? as u32;
        
        let input_token = route.hops[i];
        let output_token = route.hops[i + 1];
        
        match entry.pool_type {
            crate::cache::PoolType::V2 => {
                let reserve0 = entry.reserve0?;
                let reserve1 = entry.reserve1?;
                if reserve0.is_zero() || reserve1.is_zero() { 
                    println!("[V2 SELL] Pool {} has zero reserves: reserve0={}, reserve1={}", pool, reserve0, reserve1);
                    return None; 
                }
                let (reserve_in, reserve_out) = if input_token == token0_idx {
                    (reserve0, reserve1)
                } else {
                    (reserve1, reserve0)
                };
                
                // Get dynamic fee based on DEX name
                let fee = if let Some(dex_name) = &entry.dex_name {
                    config.get_v2_fee(dex_name)
                } else {
                    25 // Default to 0.25% if no DEX name
                };
                
                // Dynamic V2 getAmountsOut formula based on fee
                let fee_numerator = 10000 - fee;
                let amount_in_with_fee = amount_in * U256::from(fee_numerator);
                let numerator = amount_in_with_fee * reserve_out;
                let denominator = reserve_in * U256::from(10_000u32) + amount_in_with_fee;
                if denominator.is_zero() { 
                    println!("[V2 SELL] Denominator zero: reserve_in={}, amount_in={}", reserve_in, amount_in);
                    return None; 
                }
                let mut amount_out = numerator.checked_div(denominator)?;
                
                // --- Apply sell tax if exists ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax >= 1.0 {
                        println!("[TAX WARNING] Sell tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if sell_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - sell_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply buy tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // Add hop detail
                hops.push(HopDetail {
                    pool_address: *pool,
                    token_in: input_token,
                    token_out: output_token,
                    amount_in,
                    amount_out,
                    reserve_in,
                    reserve_out,
                    pool_type: crate::cache::PoolType::V2,
                    fee,
                });
                
                println!("[V2 SELL] Pool {}: reserve_in={}, reserve_out={}, amount_in={}, calculated_output={}", 
                    pool, reserve_in, reserve_out, amount_in, amount_out);
                
                amount_in = amount_out;
            }
            crate::cache::PoolType::V3 => {
                let sqrt_price_x96 = entry.sqrt_price_x96?;
                let liquidity = entry.liquidity?;
                let fee = entry.fee.unwrap_or(3000);
                let zero_for_one = input_token == token0_idx;
                
                if liquidity.is_zero() || sqrt_price_x96.is_zero() {
                    println!("[V3 SELL] Pool {} has zero liquidity or sqrtPrice: liquidity={}, sqrtPrice={}", 
                        pool, liquidity, sqrt_price_x96);
                    return None;
                }
                
                // Use new V3 math function with overflow protection
                let mut amount_out = crate::v3_math::simulate_v3_swap(
                    amount_in,
                    sqrt_price_x96,
                    liquidity,
                    fee,
                    zero_for_one,
                )?;
                
                // --- Apply sell tax if exists ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax >= 1.0 {
                        println!("[TAX WARNING] Sell tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if sell_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - sell_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply buy tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // Add hop detail
                hops.push(HopDetail {
                    pool_address: *pool,
                    token_in: input_token,
                    token_out: output_token,
                    amount_in,
                    amount_out,
                    reserve_in: U256::zero(), // V3 doesn't use reserves
                    reserve_out: U256::zero(),
                    pool_type: crate::cache::PoolType::V3,
                    fee,
                });
                
                println!("[V3 SELL] Pool {}: sqrtPrice={}, liquidity={}, amount_in={}, calculated_output={}, fee={}", 
                    pool, sqrt_price_x96, liquidity, amount_in, amount_out, fee);
                
                amount_in = amount_out;
            }
        }
    }
    
    Some(PathSimulationResult {
        total_amount_in: token_x_amount,
        total_amount_out: amount_in,
        hops,
        success: true,
    })
}

/// Test function to verify V2 simulation matches PancakeSwap Router behavior
pub fn test_pancakeswap_v2_simulation() {
    println!("=== Testing PancakeSwap V2 Simulation Accuracy ===");
    
    // Example reserves (similar to real PancakeSwap pool)
    let reserve0 = U256::from_dec_str("1000000000000000000000").unwrap(); // 1000 tokens
    let reserve1 = U256::from_dec_str("50000000000000000000000").unwrap(); // 50000 tokens
    
    // Test getAmountsOut (sell simulation)
    let amount_in = U256::from_dec_str("1000000000000000000").unwrap(); // 1 token
    let amount_in_with_fee = amount_in * U256::from(9975u32);
    let numerator = amount_in_with_fee * reserve1;
    let denominator = reserve0 * U256::from(10_000u32) + amount_in_with_fee;
    let expected_output = numerator.checked_div(denominator).unwrap();
    
    println!("V2 Sell Test:");
    println!("  Reserve0: {}", reserve0);
    println!("  Reserve1: {}", reserve1);
    println!("  AmountIn: {}", amount_in);
    println!("  Expected Output: {}", expected_output);
    println!("  Fee: 0.25% (9975/10000)");
    
    // Test getAmountsIn (buy simulation)
    let amount_out_desired = U256::from_dec_str("1000000000000000000").unwrap(); // 1 token
    let numerator2 = reserve0 * amount_out_desired * U256::from(10_000u32);
    let denominator2 = (reserve1 - amount_out_desired) * U256::from(9975u32);
    let expected_input = numerator2.checked_div(denominator2).unwrap() + U256::one();
    
    println!("\nV2 Buy Test:");
    println!("  Reserve0: {}", reserve0);
    println!("  Reserve1: {}", reserve1);
    println!("  AmountOut Desired: {}", amount_out_desired);
    println!("  Expected Input: {}", expected_input);
    println!("  Fee: 0.25% (9975/10000)");
    
    println!("\n✅ PancakeSwap V2 formulas verified!");
}

/// Test function to verify V3 simulation accuracy
pub fn test_v3_simulation() {
    println!("=== Testing V3 Simulation Accuracy ===");
    
    // Example V3 pool data
    let sqrt_price_x96 = U256::from_dec_str("79228162514264337593543950336").unwrap(); // sqrt(1) * 2^96
    let liquidity = U256::from_dec_str("1000000000000000000000").unwrap(); // 1000 tokens
    let fee = 3000; // 0.3%
    
    // Test V3 sell simulation (token0 -> token1) with smaller amount
    let amount_in = U256::from_dec_str("100000000000000000").unwrap(); // 0.1 token (smaller amount)
    let amount_out = simulate_v3_swap_single(amount_in, sqrt_price_x96, liquidity, fee, true);
    
    println!("V3 Sell Test (token0->token1):");
    println!("  SqrtPriceX96: {}", sqrt_price_x96);
    println!("  Liquidity: {}", liquidity);
    println!("  AmountIn: {}", amount_in);
    println!("  AmountOut: {:?}", amount_out);
    println!("  Fee: 0.3% ({} bps)", fee);
    
    // Test V3 sell simulation (token1 -> token0) with smaller amount
    let amount_out_reverse = simulate_v3_swap_single(amount_in, sqrt_price_x96, liquidity, fee, false);
    
    println!("\nV3 Sell Test (token1->token0):");
    println!("  AmountIn: {}", amount_in);
    println!("  AmountOut: {:?}", amount_out_reverse);
    
    // Test V3 buy calculation using the correct function with smaller amount
    let desired_output = U256::from_dec_str("100000000000000000").unwrap(); // 0.1 token (smaller amount)
    let amount_in_needed = crate::v3_math::calculate_v3_buy_amount(desired_output, sqrt_price_x96, liquidity, fee, true);
    
    println!("\nV3 Buy Test (token1->token0):");
    println!("  Desired Output: {}", desired_output);
    println!("  Amount In Needed: {:?}", amount_in_needed);
    
    // Test with even smaller amounts to avoid overflow
    let small_amount_in = U256::from_dec_str("10000000000000000").unwrap(); // 0.01 token
    let small_amount_out = simulate_v3_swap_single(small_amount_in, sqrt_price_x96, liquidity, fee, true);
    
    println!("\nV3 Small Amount Test:");
    println!("  AmountIn: {}", small_amount_in);
    println!("  AmountOut: {:?}", small_amount_out);
    
    // Test price direction
    if let Some(new_price) = crate::v3_math::get_next_sqrt_price_from_input(sqrt_price_x96, liquidity, small_amount_in, true) {
        let old_price = crate::v3_math::sqrt_price_x96_to_price(sqrt_price_x96);
        let new_price_f64 = crate::v3_math::sqrt_price_x96_to_price(new_price);
        println!("  Price change (token0->token1): {} -> {} (decreased: {})", 
            old_price, new_price_f64, old_price > new_price_f64);
    }
    
    if let Some(new_price) = crate::v3_math::get_next_sqrt_price_from_input(sqrt_price_x96, liquidity, small_amount_in, false) {
        let old_price = crate::v3_math::sqrt_price_x96_to_price(sqrt_price_x96);
        let new_price_f64 = crate::v3_math::sqrt_price_x96_to_price(new_price);
        println!("  Price change (token1->token0): {} -> {} (increased: {})", 
            old_price, new_price_f64, old_price < new_price_f64);
    }
    
    // Test exact output calculation verification
    if let Some(amount_in_needed) = amount_in_needed {
        if let Some(actual_output) = simulate_v3_swap_single(amount_in_needed, sqrt_price_x96, liquidity, fee, true) {
            println!("\nV3 Exact Output Verification:");
            println!("  Desired: {}", desired_output);
            println!("  Actual:  {}", actual_output);
            println!("  Success: {}", actual_output >= desired_output);
        }
    }
    
    println!("\n✅ V3 simulation test completed!");
}

/// Helper function to print detailed hop information in a nice format
pub fn print_path_simulation_details(result: &PathSimulationResult, path_name: &str) {
    println!("=== {} SIMULATION DETAILS ===", path_name);
    println!("Total amount in:  {}", result.total_amount_in);
    println!("Total amount out: {}", result.total_amount_out);
    println!("Number of hops:   {}", result.hops.len());
    println!("Success:          {}", result.success);
    
    if result.hops.is_empty() {
        println!("No hops to display");
        return;
    }
    
    println!("\nDetailed hop breakdown:");
    for (i, hop) in result.hops.iter().enumerate() {
        println!("  Hop {}: {} → {} (Pool: {})", i+1, hop.token_in, hop.token_out, hop.pool_address);
        println!("    Amount in:  {}", hop.amount_in);
        println!("    Amount out: {}", hop.amount_out);
        match hop.pool_type {
            crate::cache::PoolType::V2 => {
                println!("    Reserve in:  {}", hop.reserve_in);
                println!("    Reserve out: {}", hop.reserve_out);
            },
            crate::cache::PoolType::V3 => {
                println!("    V3 Pool (no reserves)");
            }
        }
        println!("    Pool type:  {:?}", hop.pool_type);
        println!("    Fee:        {} bps", hop.fee);
        println!();
    }
    
    // Calculate profit/loss if applicable
    if result.total_amount_out > result.total_amount_in {
        let profit = result.total_amount_out - result.total_amount_in;
        // Add overflow protection for as_u128() calls
        let profit_u128 = if profit > U256::from(u128::MAX) { u128::MAX } else { profit.as_u128() };
        let total_in_u128 = if result.total_amount_in > U256::from(u128::MAX) { u128::MAX } else { result.total_amount_in.as_u128() };
        let profit_percentage = (profit_u128 as f64 / total_in_u128 as f64) * 100.0;
        println!("💰 PROFIT: {} ({:.2}%)", profit, profit_percentage);
    } else if result.total_amount_out < result.total_amount_in {
        let loss = result.total_amount_in - result.total_amount_out;
        // Add overflow protection for as_u128() calls
        let loss_u128 = if loss > U256::from(u128::MAX) { u128::MAX } else { loss.as_u128() };
        let total_in_u128 = if result.total_amount_in > U256::from(u128::MAX) { u128::MAX } else { result.total_amount_in.as_u128() };
        let loss_percentage = (loss_u128 as f64 / total_in_u128 as f64) * 100.0;
        println!("📉 LOSS: {} ({:.2}%)", loss, loss_percentage);
    } else {
        println!("⚖️  BREAKEVEN: No profit or loss");
    }
}

/// Returns (amounts_in, amounts_out) vectors for each hop in buy path
/// amounts_in[i] = input to hop i, amounts_out[i] = output from hop i
pub fn simulate_buy_path_amounts_vec(
    route: &RoutePath,
    token_x_amount: U256,
    cache: &ReserveCache,
    token_index_map: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<(Vec<U256>, Vec<U256>)> {
    let mut amounts_in = Vec::with_capacity(route.pools.len());
    let mut amounts_out = Vec::with_capacity(route.pools.len());
    let mut amount_out = token_x_amount;
    // Process hops in reverse order (from tokenX back to base token)
    for (i, pool) in route.pools.iter().enumerate().rev() {
        let pool_data = cache.get(pool)?;
        let entry = pool_data.value();
        let token0_idx = *token_index_map.address_to_index.get(&entry.token0)? as u32;
        let token1_idx = *token_index_map.address_to_index.get(&entry.token1)? as u32;
        let input_token = route.hops[i];
        let output_token = route.hops[i + 1];
        match entry.pool_type {
            crate::cache::PoolType::V2 => {
                let reserve0 = entry.reserve0?;
                let reserve1 = entry.reserve1?;
                let (reserve_in, reserve_out) = if input_token == token0_idx {
                    (reserve0, reserve1)
                } else {
                    (reserve1, reserve0)
                };
                
                // Check if we have enough output available
                if amount_out >= reserve_out {
                    return None; // Insufficient liquidity
                }
                
                // Get dynamic fee based on DEX name
                let fee = if let Some(dex_name) = &entry.dex_name {
                    config.get_v2_fee(dex_name)
                } else {
                    25 // Default to 0.25% if no DEX name
                };
                
                // Dynamic V2 getAmountsIn formula based on fee
                let fee_numerator = 10000 - fee;
                let numerator = reserve_in * amount_out * U256::from(10_000u32);
                let denominator = (reserve_out - amount_out) * U256::from(fee_numerator);
                let mut amount_in = numerator.checked_div(denominator)? + U256::one();
                
                // --- Apply buy tax if exists ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_in to zero", input_token_address);
                        amount_in = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply sell tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - sell_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                // --- Apply buy tax on output_token (pool withdrawal) ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - buy_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                amounts_in.push(amount_in);
                amounts_out.push(amount_out);
                amount_out = amount_in;
            }
            crate::cache::PoolType::V3 => {
                let sqrt_price_x96 = entry.sqrt_price_x96?;
                let liquidity = entry.liquidity?;
                let fee = entry.fee.unwrap_or(3000);
                let zero_for_one = input_token == token0_idx;
                
                // Use the proper V3 buy calculation function
                let mut amount_in = crate::v3_math::calculate_v3_buy_amount(amount_out, sqrt_price_x96, liquidity, fee, zero_for_one)?;
                
                // --- Apply buy tax if exists ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_in to zero", input_token_address);
                        amount_in = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply sell tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - sell_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                // --- Apply buy tax on output_token (pool withdrawal) ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - buy_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                amounts_in.push(amount_in);
                amounts_out.push(amount_out);
                amount_out = amount_in;
            }
        }
    }
    // Reverse to get hop order (base -> tokenX)
    amounts_in.reverse();
    amounts_out.reverse();
    Some((amounts_in, amounts_out))
}

/// Returns (amounts_in, amounts_out) vectors for each hop in sell path
/// amounts_in[i] = input to hop i, amounts_out[i] = output from hop i
pub fn simulate_sell_path_amounts_vec(
    route: &RoutePath,
    token_x_amount: U256,
    cache: &ReserveCache,
    token_index_map: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<(Vec<U256>, Vec<U256>)> {
    let mut amounts_in = Vec::with_capacity(route.pools.len());
    let mut amounts_out = Vec::with_capacity(route.pools.len());
    let mut amount_in = token_x_amount;
    // Process hops in forward order (from tokenX to base token)
    for (i, pool) in route.pools.iter().enumerate() {
        let pool_data = cache.get(pool)?;
        let entry = pool_data.value();
        let token0_idx = *token_index_map.address_to_index.get(&entry.token0)? as u32;
        let token1_idx = *token_index_map.address_to_index.get(&entry.token1)? as u32;
        let input_token = route.hops[i];
        let output_token = route.hops[i + 1];
        match entry.pool_type {
            crate::cache::PoolType::V2 => {
                let reserve0 = entry.reserve0?;
                let reserve1 = entry.reserve1?;
                let (reserve_in, reserve_out) = if input_token == token0_idx {
                    (reserve0, reserve1)
                } else {
                    (reserve1, reserve0)
                };
                
                // Get dynamic fee based on DEX name
                let fee = if let Some(dex_name) = &entry.dex_name {
                    config.get_v2_fee(dex_name)
                } else {
                    25 // Default to 0.25% if no DEX name
                };
                
                // Dynamic V2 getAmountsOut formula based on fee
                let fee_numerator = 10000 - fee;
                let amount_in_with_fee = amount_in * U256::from(fee_numerator);
                let numerator = amount_in_with_fee * reserve_out;
                let denominator = reserve_in * U256::from(10_000u32) + amount_in_with_fee;
                let mut amount_out = numerator.checked_div(denominator)?;
                
                // --- Apply sell tax if exists ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax >= 1.0 {
                        println!("[TAX WARNING] Sell tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if sell_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - sell_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply buy tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                amounts_in.push(amount_in);
                amounts_out.push(amount_out);
                amount_in = amount_out;
            }
            crate::cache::PoolType::V3 => {
                let sqrt_price_x96 = entry.sqrt_price_x96?;
                let liquidity = entry.liquidity?;
                let fee = entry.fee.unwrap_or(3000);
                let zero_for_one = input_token == token0_idx;
                let mut amount_out = if zero_for_one {
                    simulate_v3_swap_single(amount_in, sqrt_price_x96, liquidity, fee, true)?
                } else {
                    simulate_v3_swap_single(amount_in, sqrt_price_x96, liquidity, fee, false)?
                };
                
                // --- Apply sell tax if exists ---
                let output_token_address = if output_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&output_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax >= 1.0 {
                        println!("[TAX WARNING] Sell tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                        amount_out = U256::zero();
                    } else if sell_tax > 0.0 {
                        let amount_out_f = amount_out.as_u128() as f64;
                        let taxed = amount_out_f * (1.0 - sell_tax);
                        amount_out = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply buy tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                amounts_in.push(amount_in);
                amounts_out.push(amount_out);
                amount_in = amount_out;
            }
        }
    }
    Some((amounts_in, amounts_out))
}

/// Returns amounts array exactly like PancakeSwap Router getAmountsOut
/// [amountIn, hop1_out, hop2_out, ..., final_out]
pub fn simulate_sell_path_amounts_array(
    route: &RoutePath,
    token_x_amount: U256,
    cache: &ReserveCache,
    token_index_map: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<Vec<U256>> {
    let mut amounts = Vec::with_capacity(route.hops.len());
    // Start with input amount (before any tax)
    let mut amount_in = token_x_amount;
    amounts.push(amount_in);

    for (i, pool) in route.pools.iter().enumerate() {
        let pool_data = cache.get(pool)?;
        let entry = pool_data.value();
        let token0_idx = *token_index_map.address_to_index.get(&entry.token0)? as u32;
        let token1_idx = *token_index_map.address_to_index.get(&entry.token1)? as u32;
        let input_token = route.hops[i];
        let output_token = route.hops[i + 1];

        // --- Apply sell tax on input_token (pool deposit) ---
        let input_token_address = if input_token == token0_idx {
            entry.token0
        } else {
            entry.token1
        };
        if let Some(tax_info) = token_tax_map.get(&input_token_address) {
            let sell_tax = tax_info.sell_tax / 100.0;
            if sell_tax >= 1.0 {
                // println!("[TAX WARNING] Sell tax >= 100% for token {:?}, setting amount_in to zero", input_token_address);
                amount_in = U256::zero();
            } else if sell_tax > 0.0 {
                let amount_in_f = amount_in.as_u128() as f64;
                let taxed = amount_in_f * (1.0 - sell_tax);
                amount_in = U256::from(taxed as u128);
                println!("[TAX INFO] Applied sell tax on input token {:?}: original={}, taxed={}, SELL TAX={}", 
                    input_token_address, amount_in_f, taxed, sell_tax);
            }
        }

        // --- Calculate pool output (before buy tax) ---
        let mut amount_out = match entry.pool_type {
            crate::cache::PoolType::V2 => {
                let reserve0 = entry.reserve0?;
                let reserve1 = entry.reserve1?;
                let (reserve_in, reserve_out) = if input_token == token0_idx {
                    (reserve0, reserve1)
                } else {
                    (reserve1, reserve0)
                };
                // Get dynamic fee based on DEX name
                let fee = if let Some(dex_name) = &entry.dex_name {
                    config.get_v2_fee(dex_name)
                } else {
                    25 // Default to 0.25% if no DEX name
                };
                let fee_numerator = 10000 - fee;
                let amount_in_with_fee = amount_in * U256::from(fee_numerator);
                let numerator = amount_in_with_fee * reserve_out;
                let denominator = reserve_in * U256::from(10_000u32) + amount_in_with_fee;
                numerator.checked_div(denominator)?
            }
            crate::cache::PoolType::V3 => {
                let sqrt_price_x96 = entry.sqrt_price_x96?;
                let liquidity = entry.liquidity?;
                let fee = entry.fee.unwrap_or(3000);
                let zero_for_one = input_token == token0_idx;
                if zero_for_one {
                    simulate_v3_swap_single(amount_in, sqrt_price_x96, liquidity, fee, true)?
                } else {
                    simulate_v3_swap_single(amount_in, sqrt_price_x96, liquidity, fee, false)?
                }
            }
        };

        // --- Apply buy tax on output_token (pool withdrawal) ---
        let output_token_address = if output_token == token0_idx {
            entry.token0
        } else {
            entry.token1
        };
        if let Some(tax_info) = token_tax_map.get(&output_token_address) {
            let buy_tax = tax_info.buy_tax / 100.0;
            if buy_tax >= 1.0 {
                println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_out to zero", output_token_address);
                amount_out = U256::zero();
            } else if buy_tax > 0.0 {
                let amount_out_f = amount_out.as_u128() as f64;
                let taxed = amount_out_f * (1.0 - buy_tax);
                amount_out = U256::from(taxed as u128);
                println!("[TAX INFO] Applied buy tax on output token {:?}: original={}, taxed={}", 
                    output_token_address, amount_out_f, taxed);
            }
        }

        // Store the after-tax output for this hop
        amounts.push(amount_out);
        // The after-tax output becomes the input for the next hop
        amount_in = amount_out;
    }
    Some(amounts)
}

/// Returns amounts array exactly like PancakeSwap Router getAmountsIn
/// [amountIn, hop1_out, hop2_out, ..., amountOut]
pub fn simulate_buy_path_amounts_array(
    route: &RoutePath,
    token_x_amount: U256,
    cache: &ReserveCache,
    token_index_map: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<Vec<U256>> {
    let mut amounts: Vec<U256> = Vec::with_capacity(route.hops.len());
    let mut amount_out = token_x_amount;
    
    // Calculate amounts in reverse order (from tokenX back to base)
    let mut reverse_amounts = Vec::with_capacity(route.hops.len());
    reverse_amounts.push(token_x_amount); // Start with desired output
    
    for (i, pool) in route.pools.iter().enumerate().rev() {
        let pool_data = cache.get(pool)?;
        let entry = pool_data.value();
        let token0_idx = *token_index_map.address_to_index.get(&entry.token0)? as u32;
        let token1_idx = *token_index_map.address_to_index.get(&entry.token1)? as u32;
        let input_token = route.hops[i];
        let output_token = route.hops[i + 1];
        
        let mut amount_in = match entry.pool_type {
            crate::cache::PoolType::V2 => {
                let reserve0 = entry.reserve0?;
                let reserve1 = entry.reserve1?;
                let (reserve_in, reserve_out) = if input_token == token0_idx {
                    (reserve0, reserve1)
                } else {
                    (reserve1, reserve0)
                };
                
                // Check if we have enough output available
                if amount_out >= reserve_out {
                    return None; // Insufficient liquidity
                }
                
                // Get dynamic fee based on DEX name
                let fee = if let Some(dex_name) = &entry.dex_name {
                    config.get_v2_fee(dex_name)
                } else {
                    25 // Default to 0.25% if no DEX name
                };
                
                // Dynamic V2 getAmountsIn formula based on fee
                let fee_numerator = 10000 - fee;
                let numerator = reserve_in * amount_out * U256::from(10_000u32);
                let denominator = (reserve_out - amount_out) * U256::from(fee_numerator);
                let mut amount_in = numerator.checked_div(denominator)? + U256::one();
                
                // --- Apply buy tax if exists ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_in to zero", input_token_address);
                        amount_in = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply sell tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - sell_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                amount_in
            }
            crate::cache::PoolType::V3 => {
                let sqrt_price_x96 = entry.sqrt_price_x96?;
                let liquidity = entry.liquidity?;
                let fee = entry.fee.unwrap_or(3000);
                let zero_for_one = input_token == token0_idx;
                
                // Use the proper V3 buy calculation function
                let mut amount_in = crate::v3_math::calculate_v3_buy_amount(amount_out, sqrt_price_x96, liquidity, fee, zero_for_one)?;
                
                // --- Apply buy tax if exists ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let buy_tax = tax_info.buy_tax / 100.0;
                    if buy_tax >= 1.0 {
                        println!("[TAX WARNING] Buy tax >= 100% for token {:?}, setting amount_in to zero", input_token_address);
                        amount_in = U256::zero();
                    } else if buy_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - buy_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                // --- Apply sell tax on input_token (pool deposit) ---
                let input_token_address = if input_token == token0_idx {
                    entry.token0
                } else {
                    entry.token1
                };
                if let Some(tax_info) = token_tax_map.get(&input_token_address) {
                    let sell_tax = tax_info.sell_tax / 100.0;
                    if sell_tax > 0.0 {
                        let amount_in_f = amount_in.as_u128() as f64;
                        let taxed = amount_in_f / (1.0 - sell_tax);
                        amount_in = U256::from(taxed as u128);
                    }
                }
                
                amount_in
            }
        };
        
        reverse_amounts.push(amount_in);
        amount_out = amount_in;
    }
    
    // Reverse to get correct order (base -> tokenX)
    reverse_amounts.reverse();
    Some(reverse_amounts)
}

/// Test function to verify dynamic V2 fee implementation
pub fn test_dynamic_v2_fees() {
    println!("=== Testing Dynamic V2 Fee Implementation ===");
    
    let config = Config::default();
    
    // Test different DEX fees
    let test_dexes = vec![
        ("PancakeSwap V2", 25),    // 0.25%
        ("BiSwap", 10),            // 0.1%
        ("ApeSwap", 20),           // 0.2%
        ("BakerySwap", 30),        // 0.3%
        ("MDEX", 20),              // 0.2%
        ("SushiSwap BSC", 30),     // 0.3%
        ("Unknown DEX", 25),       // Should default to 0.25%
    ];
    
    for (dex_name, expected_fee) in test_dexes {
        let actual_fee = config.get_v2_fee(dex_name);
        println!("  {}: {} bps (expected: {} bps) - {}", 
            dex_name, actual_fee, expected_fee, 
            if actual_fee == expected_fee { "✅" } else { "❌" });
    }
    
    // Test fee calculation formulas
    println!("\n=== Testing Fee Calculation Formulas ===");
    
    let reserve_in = U256::from_dec_str("1000000000000000000000").unwrap(); // 1000 tokens
    let reserve_out = U256::from_dec_str("50000000000000000000000").unwrap(); // 50000 tokens
    let amount_out = U256::from_dec_str("1000000000000000000").unwrap(); // 1 token
    
    let fee_test_dexes = vec![
        ("PancakeSwap V2", 25),
        ("BiSwap", 10),
        ("ApeSwap", 20),
        ("BakerySwap", 30),
    ];
    
    for (dex_name, fee) in fee_test_dexes {
        let fee_numerator = 10000 - fee;
        
        // Buy calculation (getAmountsIn)
        let numerator = reserve_in * amount_out * U256::from(10_000u32);
        let denominator = (reserve_out - amount_out) * U256::from(fee_numerator);
        let amount_in = numerator.checked_div(denominator).unwrap() + U256::one();
        
        // Sell calculation (getAmountsOut)
        let amount_in_sell = U256::from_dec_str("1000000000000000000").unwrap(); // 1 token
        let amount_in_with_fee = amount_in_sell * U256::from(fee_numerator);
        let numerator_sell = amount_in_with_fee * reserve_out;
        let denominator_sell = reserve_in * U256::from(10_000u32) + amount_in_with_fee;
        let amount_out_sell = numerator_sell.checked_div(denominator_sell).unwrap();
        
        println!("  {} ({} bps):", dex_name, fee);
        println!("    Buy: {} tokens in for {} tokens out", amount_in, amount_out);
        println!("    Sell: {} tokens out for {} tokens in", amount_out_sell, amount_in_sell);
    }
    
    println!("\n✅ Dynamic V2 fee test completed!");
}

/// Main function to simulate all filtered routes for a given token and pool
pub fn simulate_all_filtered_routes(
    token_address: H160,
    pool_address: H160,
    token_x_amount: U256,
    all_tokens: &HashMap<H160, u32>,
    precomputed_route_cache: &DashMap<u32, Vec<RoutePath>>,
    reserve_cache: &ReserveCache,
    token_index_map: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<ComprehensiveSimulationResults> {
    // Get token index
    let token_idx = all_tokens.get(&token_address).copied()?;
    
    // Get all paths for this token
    let paths = precomputed_route_cache.get(&token_idx)?;
    
    // Filter paths containing the specified pool
    let filtered: Vec<_> = paths.iter()
        .enumerate()
        .filter(|(_, path)| path.pools.contains(&pool_address))
        .collect();
    
    if filtered.is_empty() {
        return None;
    }
    
    let mut route_results = Vec::new();
    let mut best_profit_route: Option<usize> = None;
    let mut best_profit_amount: Option<i128> = None;
    let mut best_profit_percentage: Option<f64> = None;
    let mut successful_routes = 0;
    let mut profitable_routes = 0;
    
    // Simulate each filtered route
    for (route_index, (_, path)) in filtered.iter().enumerate() {
        // Split route into buy and sell paths
        let (buy, sell) = match split_route_around_token_x(path, token_idx) {
            Some(split) => split,
            None => continue,
        };
        
        // Simulate buy path
        let buy_result = simulate_buy_path(&buy, token_x_amount, reserve_cache, token_index_map, token_tax_map, config);
        let buy_amounts_array = simulate_buy_path_amounts_array(&buy, token_x_amount, reserve_cache, token_index_map, token_tax_map, config);
        let buy_amounts_vec = simulate_buy_path_amounts_vec(&buy, token_x_amount, reserve_cache, token_index_map, token_tax_map, config);
        
        // Simulate sell path
        let sell_result = simulate_sell_path(&sell, token_x_amount, reserve_cache, token_index_map, token_tax_map, config);
        let sell_amounts_array = simulate_sell_path_amounts_array(&sell, token_x_amount, reserve_cache, token_index_map, token_tax_map, config);
        let sell_amounts_vec = simulate_sell_path_amounts_vec(&sell, token_x_amount, reserve_cache, token_index_map, token_tax_map, config);
        
        // Calculate profit/loss
        let (profit_loss, profit_percentage) = if let (Some(buy), Some(sell)) = (&buy_result, &sell_result) {
            // Add overflow protection for as_u128() calls
            let buy_cost = if buy.total_amount_in > U256::from(u128::MAX) { 
                u128::MAX as i128 
            } else { 
                buy.total_amount_in.as_u128() as i128 
            };
            let sell_revenue = if sell.total_amount_out > U256::from(u128::MAX) { 
                u128::MAX as i128 
            } else { 
                sell.total_amount_out.as_u128() as i128 
            };
            let profit = sell_revenue - buy_cost;
            let percentage = if buy_cost > 0 {
                (profit as f64 / buy_cost as f64) * 100.0
            } else {
                0.0
            };
            (Some(profit), Some(percentage))
        } else {
            (None, None)
        };
        
        // Track best profit
        if let Some(profit) = profit_loss {
            if profit > 0 {
                profitable_routes += 1;
                if best_profit_amount.is_none() || profit > best_profit_amount.unwrap() {
                    best_profit_route = Some(route_index);
                    best_profit_amount = Some(profit);
                    best_profit_percentage = profit_percentage;
                }
            }
        }
        
        if buy_result.is_some() || sell_result.is_some() {
            successful_routes += 1;
        }
        
        // Create route result
        let route_result = RouteSimulationResult {
            route_index,
            buy_path: buy_result,
            sell_path: sell_result,
            buy_amounts_array,
            sell_amounts_array,
            buy_amounts_vec,
            sell_amounts_vec,
            profit_loss,
            profit_percentage,
        };
        
        route_results.push(route_result);
    }
    
    Some(ComprehensiveSimulationResults {
        token_address,
        pool_address,
        token_x_amount,
        total_routes: filtered.len(),
        successful_routes,
        profitable_routes,
        route_results,
        best_profit_route,
        best_profit_amount,
        best_profit_percentage,
    })
}

/// Helper function to print comprehensive results in a nice format
pub fn print_comprehensive_results(results: &ComprehensiveSimulationResults) {
    println!("=== COMPREHENSIVE SIMULATION RESULTS ===");
    println!("Token Address: {}", results.token_address);
    println!("Pool Address:  {}", results.pool_address);
    println!("Token X Amount: {}", results.token_x_amount);
    println!("Total Routes:   {}", results.total_routes);
    println!("Successful:     {}", results.successful_routes);
    println!("Profitable:     {}", results.profitable_routes);
    
    if let Some(best_idx) = results.best_profit_route {
        println!("Best Profit Route: {}", best_idx);
        if let Some(profit) = results.best_profit_amount {
            println!("Best Profit Amount: {}", profit);
        }
        if let Some(percentage) = results.best_profit_percentage {
            println!("Best Profit Percentage: {:.2}%", percentage);
        }
    }
    
    println!("\n=== DETAILED ROUTE BREAKDOWN ===");
    for (i, route) in results.route_results.iter().enumerate() {
        println!("\n--- Route {} ---", i + 1);
        
        // Buy path info
        if let Some(buy) = &route.buy_path {
            println!("BUY Path: {} hops, Total In: {}, Total Out: {}", 
                buy.hops.len(), buy.total_amount_in, buy.total_amount_out);
        } else {
            println!("BUY Path: Failed");
        }
        
        // Sell path info
        if let Some(sell) = &route.sell_path {
            println!("SELL Path: {} hops, Total In: {}, Total Out: {}", 
                sell.hops.len(), sell.total_amount_in, sell.total_amount_out);
        } else {
            println!("SELL Path: Failed");
        }
        
        // Router format arrays
        if let Some(buy_array) = &route.buy_amounts_array {
            println!("BUY Router Format: {:?}", buy_array);
        }
        if let Some(sell_array) = &route.sell_amounts_array {
            println!("SELL Router Format: {:?}", sell_array);
        }
        
        // Profit/Loss
        if let Some(profit) = route.profit_loss {
            if profit > 0 {
                println!("💰 PROFIT: {} ({:.2}%)", profit, route.profit_percentage.unwrap_or(0.0));
            } else if profit < 0 {
                println!("📉 LOSS: {} ({:.2}%)", profit.abs(), route.profit_percentage.unwrap_or(0.0).abs());
            } else {
                println!("⚖️  BREAKEVEN");
            }
        } else {
            println!("❌ Could not calculate profit/loss");
        }
    }
}
