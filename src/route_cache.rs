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
    Other(String),
}

#[derive(Clone, Debug)]
pub struct PoolMeta {
    pub token0: H160,
    pub token1: H160,
    pub address: H160,
    pub dex_type: DEXType,
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
        for (_, x_idx) in &all_tokens_vec {
            if *x_idx == base_idx { continue; }
            
            // base -> x
            if let Some(&(pool1, _)) = pool_lookup.get(&(base_idx, *x_idx)) {
                // x -> base
                if let Some(&(pool2, _)) = pool_lookup.get(&(*x_idx, base_idx)) {
                    let path = RoutePath {
                        hops: vec![base_idx, *x_idx, base_idx],
                        pools: vec![pool1.address, pool2.address],
                        dex_types: vec![pool1.dex_type.clone(), pool2.dex_type.clone()],
                    };
                    token_to_paths.entry(*x_idx).or_default().insert(path);
                }
            }
        }
        
        // 3-hop: base -> X -> Y -> base
        for (_, x_idx) in &all_tokens_vec {
            if *x_idx == base_idx { continue; }
            
            for (_, y_idx) in &all_tokens_vec {
                if *y_idx == base_idx || *y_idx == *x_idx { continue; }
                
                // base -> x
                if let Some(&(pool1, _)) = pool_lookup.get(&(base_idx, *x_idx)) {
                    // x -> y
                    if let Some(&(pool2, _)) = pool_lookup.get(&(*x_idx, *y_idx)) {
                        // y -> base
                        if let Some(&(pool3, _)) = pool_lookup.get(&(*y_idx, base_idx)) {
                            let path = RoutePath {
                                hops: vec![base_idx, *x_idx, *y_idx, base_idx],
                                pools: vec![pool1.address, pool2.address, pool3.address],
                                dex_types: vec![pool1.dex_type.clone(), pool2.dex_type.clone(), pool3.dex_type.clone()],
                            };
                            token_to_paths.entry(*x_idx).or_default().insert(path.clone());
                            token_to_paths.entry(*y_idx).or_default().insert(path);
                        }
                    }
                }
            }
        }
        
        // Insert results into the shared DashMap
        for (token_idx, paths) in token_to_paths {
            result.entry(token_idx).or_insert_with(Vec::new).extend(paths.into_iter());
        }
    });
    
    println!("Route cache built. Unique tokens with paths: {}", result.len());
    result
}

