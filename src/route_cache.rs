use crate::token_tax::{TokenTaxInfo};
use ethers::types::H160;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use rayon::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DEXType {
    PancakeV2,
    BiSwapV2,
    ApeSwapV2,
    BakeryV2,
    SushiV2,
    PancakeV3,
    BiSwapV3,
    ApeSwapV3,
    BakeryV3,
    SushiV3,
    Other(String),
}

#[derive(Clone, Debug)]
pub struct PoolMeta {
    pub token0: H160,
    pub token1: H160,
    pub address: H160,
    pub dex_type: DEXType,
    pub factory: Option<H160>, // V3 only
    pub fee: Option<u32>,      // V3 only
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RoutePath {
    pub hops: Vec<u32>,      // token indices
    pub pools: Vec<H160>,   // pool addresses
    pub dex_types: Vec<DEXType>,
}

/// Build a cache of all 2-hop and 3-hop arbitrage cycles for each base token using parallel processing.
pub fn build_route_cache(
    all_tokens: &HashMap<H160, u32>,
    all_pools: &[PoolMeta],
    base_tokens: &[H160],
    token_tax_info: &HashMap<H160, TokenTaxInfo>, // <-- add this argument
) -> DashMap<u32, Vec<RoutePath>> {
    println!("Building route cache for {} tokens and {} pools", all_tokens.len(), all_pools.len());
    
    // Build a quick lookup: (tokenA, tokenB) -> (pool, dex_type)
    let mut pool_lookup: HashMap<(u32, u32), (&PoolMeta, bool)> = HashMap::new();
    for pool in all_pools {
        if let (Some(&idx0), Some(&idx1)) = (all_tokens.get(&pool.token0), all_tokens.get(&pool.token1)) {
            pool_lookup.insert((idx0, idx1), (pool, true));
            pool_lookup.insert((idx1, idx0), (pool, false));
        }
    }
    
    // Convert all_tokens to Vec for parallel processing
    let all_tokens_vec: Vec<(H160, u32)> = all_tokens.iter().map(|(k, v)| (*k, *v)).collect();
    
    // Use DashMap for thread-safe concurrent insertion
    let result = DashMap::new();
    
    // Process each base token in parallel
    base_tokens.par_iter().for_each(|&base| {
        let base_idx = match all_tokens.get(&base) {
            Some(idx) => *idx,
            None => return,
        };
        
        let mut token_to_paths: HashMap<u32, HashSet<RoutePath>> = HashMap::new();
        
        // 2-hop: base -> X -> base
        let two_hop_paths: Vec<(u32, RoutePath)> = all_tokens_vec.par_iter()
            .filter_map(|&(token_addr, x_idx)| {
                if x_idx == base_idx { return None; }
                // --- Skip tokens with simulationSuccess == false ---
                if let Some(tax) = token_tax_info.get(&token_addr) {
                    if !tax.simulation_success { return None; }
                }
                if let Some(&(pool1, _)) = pool_lookup.get(&(base_idx, x_idx)) {
                    if let Some(&(pool2, _)) = pool_lookup.get(&(x_idx, base_idx)) {
                        let path = RoutePath {
                            hops: vec![base_idx, x_idx, base_idx],
                            pools: vec![pool1.address, pool2.address],
                            dex_types: vec![pool1.dex_type.clone(), pool2.dex_type.clone()],
                        };
                        return Some((x_idx, path));
                    }
                }
                None
            })
            .collect();
        for (x_idx, path) in two_hop_paths {
            token_to_paths.entry(x_idx).or_default().insert(path);
        }
        
        // 3-hop: base -> X -> Y -> base
        let three_hop_paths: Vec<((u32, u32), RoutePath)> = all_tokens_vec.par_iter()
            .flat_map_iter(|&(token_addr, x_idx)| {
                if x_idx == base_idx { return Vec::new().into_iter(); }
                // --- Skip tokens with simulationSuccess == false ---
                if let Some(tax) = token_tax_info.get(&token_addr) {
                    if !tax.simulation_success { return Vec::new().into_iter(); }
                }
                all_tokens_vec.par_iter()
                    .filter_map(|&(token_addr_y, y_idx)| {
                        if y_idx == base_idx || y_idx == x_idx { return None; }
                        // --- Skip tokens with simulationSuccess == false ---
                        if let Some(tax) = token_tax_info.get(&token_addr_y) {
                            if !tax.simulation_success { return None; }
                        }
                        if let Some(&(pool1, _)) = pool_lookup.get(&(base_idx, x_idx)) {
                            if let Some(&(pool2, _)) = pool_lookup.get(&(x_idx, y_idx)) {
                                if let Some(&(pool3, _)) = pool_lookup.get(&(y_idx, base_idx)) {
                                    let path = RoutePath {
                                        hops: vec![base_idx, x_idx, y_idx, base_idx],
                                        pools: vec![pool1.address, pool2.address, pool3.address],
                                        dex_types: vec![pool1.dex_type.clone(), pool2.dex_type.clone(), pool3.dex_type.clone()],
                                    };
                                    return Some(((x_idx, y_idx), path));
                                }
                            }
                        }
                        None
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            })
            .collect();
        for ((x_idx, y_idx), path) in three_hop_paths {
            token_to_paths.entry(x_idx).or_default().insert(path.clone());
            token_to_paths.entry(y_idx).or_default().insert(path);
        }
        
        // Insert results into the shared DashMap
        for (token_idx, paths) in token_to_paths {
            result.entry(token_idx).or_insert_with(Vec::new).extend(paths.into_iter());
        }
    });
    
    println!("Route cache built. Unique tokens with paths: {}", result.len());
    result
}

/// Build a mapping: tokenX -> baseToken -> [pools...]
pub fn build_token_to_base_token_pools(
    all_pools: &[PoolMeta],
    base_tokens: &[H160],
) -> HashMap<H160, HashMap<H160, Vec<H160>>> {
    let mut map: HashMap<H160, HashMap<H160, Vec<H160>>> = HashMap::new();
    for pool in all_pools {
        for base in base_tokens {
            if pool.token0 == *base {
                map.entry(pool.token1).or_default().entry(*base).or_default().push(pool.address);
            } else if pool.token1 == *base {
                map.entry(pool.token0).or_default().entry(*base).or_default().push(pool.address);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::H160;

    #[test]
    fn test_token_to_base_token_pools() {
        // Example token addresses (replace with real ones in production)
        let usdt = H160::from_low_u64_be(1);
        let wbnb = H160::from_low_u64_be(2);
        let cake = H160::from_low_u64_be(3);
        let busd = H160::from_low_u64_be(4);

        // Example pools (token0, token1, pool address)
        let all_pools = vec![
            PoolMeta { token0: usdt, token1: cake, address: H160::from_low_u64_be(1001), dex_type: DEXType::PancakeV2, factory: None, fee: None }, // USDT-CAKE
            PoolMeta { token0: wbnb, token1: cake, address: H160::from_low_u64_be(1002), dex_type: DEXType::PancakeV2, factory: None, fee: None }, // WBNB-CAKE
            PoolMeta { token0: busd, token1: cake, address: H160::from_low_u64_be(1003), dex_type: DEXType::PancakeV2, factory: None, fee: None }, // BUSD-CAKE
            PoolMeta { token0: wbnb, token1: usdt, address: H160::from_low_u64_be(1004), dex_type: DEXType::PancakeV2, factory: None, fee: None }, // WBNB-USDT
        ];

        // List of base tokens
        let base_tokens = vec![usdt, wbnb, busd];

        // Build the mapping
        let token_basepools = build_token_to_base_token_pools(&all_pools, &base_tokens);

        // Example: CAKE event comes in
        let affected_token = cake;
        if let Some(base_map) = token_basepools.get(&affected_token) {
            for (base, pools) in base_map {
                println!("CAKE can be traded with BASE {:?} through pools: {:?}", base, pools);
            }
        }
        // Example: Only CAKE-USDT pools
        if let Some(cake_usdt_pools) = token_basepools.get(&cake).and_then(|m| m.get(&usdt)) {
            println!("All direct CAKE-USDT pools: {:?}", cake_usdt_pools);
            assert_eq!(cake_usdt_pools, &vec![H160::from_low_u64_be(1001)]);
        }
    }
}

