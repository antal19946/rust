// File: src/arb_path_simulator.rs

use crate::route_cache::RoutePath;
use crate::cache::ReserveCache;
use crate::token_index::TokenIndexMap;
use crate::simulate_swap_path::{simulate_buy_path_amounts_array, simulate_sell_path_amounts_array};
use crate::split_route_path::split_route_around_token_x;
use crate::token_tax::TokenTaxMap;
use crate::config::Config;
use std::sync::Arc;
use dashmap::DashMap;
use ethers::types::{H160, U256};
use rayon::prelude::*;

/// Result of simulating a full arbitrage path (buy+sell) in router-style amounts array
#[derive(Debug, Clone)]
pub struct SimulatedRoute {
    pub merged_amounts: Vec<U256>,
    pub buy_amounts: Vec<U256>,      // [baseIn, ..., tokenX, ..., baseOut]
    pub sell_amounts: Vec<U256>,      // [baseIn, ..., tokenX, ..., baseOut]
    // pub merged_tokens: Vec<u32>,        // token indices for each hop
    // pub merged_symbols: Vec<String>,    // human-readable token symbols (if available)
    pub buy_symbols: Vec<String>,    // human-readable token symbols (if available)
    pub sell_symbols: Vec<String>,    // human-readable token symbols (if available)
    pub buy_pools: Vec<H160>,        // pool addresses for each hop
    pub sell_pools: Vec<H160>,        // pool addresses for each hop

    pub merged_pools: Vec<H160>,        // pool addresses for each hop
    pub profit: U256,                   // baseOut - baseIn
    pub profit_percentage: f64,         // (profit / amount_in) * 100
    pub buy_path: RoutePath,
    pub sell_path: RoutePath,
    // pub sell_test_amounts: Vec<U256>,
}

/// Helper to map token index to symbol (extend as needed)
pub fn token_index_to_symbol(idx: u32, token_index: &TokenIndexMap) -> String {
    // Try to get address, then symbol from config or fallback
    if let Some(addr) = token_index.index_to_address.get(&(idx as u32)) {
        // Return complete address instead of truncated version
        format!("0x{:x}", addr)
    } else {
        format!("token{}", idx)
    }
}

/// Simulate all arbitrage paths for tokenX and affected pool, returning router-style merged arrays
pub fn simulate_all_paths_for_token_x(
    token_x_index: u32,
    token_x_amount: U256,
    affected_pool: H160,
    route_cache: &DashMap<u32, Vec<RoutePath>>,
    reserve_cache: &ReserveCache,
    token_index: &TokenIndexMap,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Vec<SimulatedRoute> {
    let candidate_routes = route_cache
        .get(&token_x_index)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();

    candidate_routes
        .into_par_iter()
        .filter_map(|route| {
            if !route.pools.contains(&affected_pool) {
                return None;
            }
            let (buy_path, sell_path) = match split_route_around_token_x(&route, token_x_index) {
                Some(parts) => parts,
                None => return None,
            };
            let buy_amounts = simulate_buy_path_amounts_array(&buy_path, token_x_amount, reserve_cache, token_index, token_tax_map, config)?;
            let sell_amounts = simulate_sell_path_amounts_array(&sell_path, token_x_amount, reserve_cache, token_index, token_tax_map, config)?;
            if buy_amounts.is_empty() || sell_amounts.is_empty() {
                return None;
            }
            // Merge arrays: [buy_amounts..., sell_amounts[1..]]
            let mut merged_amounts = buy_amounts.clone();
            merged_amounts.extend_from_slice(&sell_amounts[1..]);
            // Defensive checks for overflow/underflow
            if merged_amounts.len() < 2
                || merged_amounts[0].is_zero()
                || merged_amounts.last().unwrap().is_zero()
                || merged_amounts.iter().any(|x| x.bits() > 128 && *x > U256::from_dec_str("1000000000000000000000000000000000000000").unwrap())
            {
                println!("⚠️  Skipping path due to invalid or suspicious amounts: {:?}", merged_amounts);
                return None;
            }
            // Merge token indices: buy_path.hops + sell_path.hops[1..]
            // let mut merged_tokens = buy_path.hops.clone();
            // merged_tokens.extend_from_slice(&sell_path.hops[1..]);
            // Map to symbols
            // let sell_test_amounts = simulate_sell_path_amounts_array(
            //     &route, 
            //     merged_amounts[0], 
            //     reserve_cache, 
            //     token_index,
            //     token_tax_map,
            //     config
            // )?;
            // let merged_symbols = merged_tokens.iter().map(|&idx| token_index_to_symbol(idx, token_index)).collect();
            // // Merge pools: buy_path.pools + sell_path.pools
            let mut merged_pools = buy_path.pools.clone();
            merged_pools.extend_from_slice(&sell_path.pools);
            // Profit: last - first (saturating to avoid panic)
            let profit = merged_amounts.last().unwrap().saturating_sub(merged_amounts[0]);
            
            // Calculate profit percentage
            let profit_percentage = if merged_amounts[0] > U256::zero() {
                let profit_f64 = profit.as_u128() as f64;
                let amount_in_f64 = merged_amounts[0].as_u128() as f64;
                (profit_f64 / amount_in_f64) * 100.0
            } else {
                0.0
            };
            
            Some(SimulatedRoute {
                merged_amounts,
                buy_amounts,
                sell_amounts,
                // merged_tokens,
                // merged_symbols,
                buy_symbols: buy_path.hops.iter().map(|&idx| token_index_to_symbol(idx, token_index)).collect(),
                sell_symbols: sell_path.hops.iter().map(|&idx| token_index_to_symbol(idx, token_index)).collect(),
                merged_pools: merged_pools.clone(),
                buy_pools: buy_path.pools.clone(),
                sell_pools: sell_path.pools.clone(),
                profit,
                profit_percentage,
                buy_path,
                sell_path,
                // sell_test_amounts,
            })
        })
        .collect()
}

// pub fn print_simulated_route(route: &SimulatedRoute) {
//     println!("Arb Path: ");
//     for ((amt, sym), idx) in route.merged_amounts.iter().zip(&route.merged_symbols).zip(0..) {
//         println!("  Step {}: {} {}", idx, amt, sym);
//     }
//     println!("  Pools: {:?}", route.merged_pools);
//     println!("  Profit: {} {} ({:.2}%)", 
//         route.profit, 
//         route.merged_symbols.first().unwrap_or(&"?".to_string()),
//         route.profit_percentage
//     );
// }
