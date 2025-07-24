// ✅ Step 1: Update Imports for Tick + Fee
use crate::token_graph::{TokenGraph, GraphEdge};
use crate::cache::{PoolType, ReserveCache};
use crate::token_index::TokenIndexMap;
// use crate::utils::{simulate_v2_swap_safe, simulate_v3_swap_precise}; // ⬅️ Updated

use primitive_types::U256;
use smallvec::SmallVec;
use ethers::types::H160;
use dashmap::DashMap;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct RoutePath {
    pub hops: SmallVec<[u32; 4]>,
    pub pools: SmallVec<[H160; 3]>,
    pub dex_types: SmallVec<[PoolType; 3]>,
    pub output: f64,
}

#[derive(Clone, Debug)]
pub struct BestRoute {
    pub best_buy: Option<RoutePath>,
    pub best_sell: Option<RoutePath>,
}

#[derive(Clone, Debug)]
pub struct PartialRoute {
    pub hops: SmallVec<[u32; 4]>,
    pub pools: SmallVec<[H160; 3]>,
    pub dex_types: SmallVec<[PoolType; 3]>,
}

pub fn dfs_all_paths(
    current: u32,
    target: u32,
    depth: usize,
    graph: &TokenGraph,
    visited: &SmallVec<[u32; 4]>,
) -> Vec<PartialRoute> {
    let mut results = Vec::new();
    if let Some(neighbors) = graph.edges.get(&current) {
        for edge in neighbors.iter() {
            if visited.contains(&edge.to) {
                continue;
            }
            let mut new_visited = visited.clone();
            new_visited.push(edge.to);
            if edge.to == target {
                results.push(PartialRoute {
                    hops: new_visited.clone(),
                    pools: SmallVec::from_vec(vec![edge.pool]),
                    dex_types: SmallVec::from_vec(vec![edge.pool_type.clone()]),
                });
            }
            if new_visited.len() - 1 < depth {
                let sub_paths = dfs_all_paths(edge.to, target, depth, graph, &new_visited);
                for path in sub_paths {
                    let mut hops = visited.clone();
                    hops.extend_from_slice(&path.hops[1..]);
                    let mut pools = SmallVec::from_vec(vec![edge.pool]);
                    pools.extend(path.pools.iter().cloned());
                    let mut dex_types = SmallVec::from_vec(vec![edge.pool_type.clone()]);
                    dex_types.extend(path.dex_types.iter().cloned());
                    results.push(PartialRoute {
                        hops,
                        pools,
                        dex_types,
                    });
                }
            }
        }
    }
    results
}

pub fn simulate_path(
    route: &PartialRoute,
    reserve_cache: &ReserveCache,
    token_index: &TokenIndexMap,
) -> f64 {
    if route.hops.len() != route.pools.len() + 1 {
        return 0.0;
    }
    let mut amount_in = 1.0_f64;
    for i in 0..route.pools.len() {
        let from_token = route.hops[i];
        let to_token = route.hops[i + 1];
        let pool = route.pools[i];
        let pool_type = route.dex_types[i].clone();
        let Some(entry) = reserve_cache.get(&pool) else {
            return 0.0;
        };
        let entry = entry.value();
        let token0_index = token_index.address_to_index.get(&entry.token0).copied().unwrap_or(0);
        let token1_index = token_index.address_to_index.get(&entry.token1).copied().unwrap_or(0);
        let is_forward = match (
            from_token == token0_index,
            to_token == token1_index,
        ) {
            (true, true) => true,
            (false, false) => false,
            _ => return 0.0,
        };

        // amount_in = match pool_type {
        //     PoolType::V2 => simulate_v2_swap_safe(
        //         amount_in,
        //         if is_forward {
        //             entry.reserve0.unwrap_or(U256::zero())
        //         } else {
        //             entry.reserve1.unwrap_or(U256::zero())
        //         },
        //         if is_forward {
        //             entry.reserve1.unwrap_or(U256::zero())
        //         } else {
        //             entry.reserve0.unwrap_or(U256::zero())
        //         },
        //         entry.fee.unwrap_or(30),
        //         is_forward,
        //     ),
        //     PoolType::V3 => simulate_v3_swap_precise(
        //         amount_in,
        //         entry.sqrt_price_x96.unwrap_or(U256::zero()),
        //         entry.liquidity.unwrap_or(U256::zero()),
        //         entry.fee.unwrap_or(30),
        //         is_forward,
        //     ),
        // };

        if amount_in <= 0.0 || amount_in.is_nan() || amount_in.is_infinite() {
            return 0.0;
        }
    }
    amount_in
}

pub fn generate_best_routes_for_token(
    token_x: u32,
    base_tokens: &[u32],
    graph: &TokenGraph,
    reserve_cache: &ReserveCache,
    token_index: &TokenIndexMap,
) -> BestRoute {
    let mut best_buy: Option<RoutePath> = None;
    let mut best_sell: Option<RoutePath> = None;
    for &base in base_tokens {
        if base == token_x {
            continue;
        }
        let mut visited = SmallVec::<[u32; 4]>::new();
        visited.push(base);
        let buy_routes = dfs_all_paths(base, token_x, 2, graph, &visited);
        for route in buy_routes.iter() {
            let output = simulate_path(route, reserve_cache, token_index);
            if best_buy.is_none() || output > best_buy.as_ref().unwrap().output {
                best_buy = Some(RoutePath {
                    hops: route.hops.clone(),
                    pools: route.pools.clone(),
                    dex_types: route.dex_types.clone(),
                    output,
                });
            }
        }
        let mut visited = SmallVec::<[u32; 4]>::new();
        visited.push(token_x);
        let sell_routes = dfs_all_paths(token_x, base, 2, graph, &visited);
        for route in sell_routes.iter() {
            let output = simulate_path(route, reserve_cache, token_index);
            if best_sell.is_none() || output > best_sell.as_ref().unwrap().output {
                best_sell = Some(RoutePath {
                    hops: route.hops.clone(),
                    pools: route.pools.clone(),
                    dex_types: route.dex_types.clone(),
                    output,
                });
            }
        }
    }
    BestRoute { best_buy, best_sell }
}

pub fn populate_best_routes_for_all_tokens(
    graph: &TokenGraph,
    reserve_cache: &ReserveCache,
    token_index: &TokenIndexMap,
    base_tokens: &[u32],
    tracked_tokens: &[u32],
    route_cache: &DashMap<u32, BestRoute>,
) {
    tracked_tokens.par_iter().for_each(|&token_x| {
        let result = generate_best_routes_for_token(
            token_x,
            base_tokens,
            graph,
            reserve_cache,
            token_index,
        );
        route_cache.insert(token_x, result);
    });
}
