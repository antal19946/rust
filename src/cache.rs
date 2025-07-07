use dashmap::DashMap;
use ethers::types::H160;
use primitive_types::U256;
use std::collections::HashMap;
use std::collections::HashSet;
use crate::fetch_pairs::PairInfo;
use crate::config::DexVersion;
use crate::bindings::{UniswapV2Pair, UniswapV3Pool};
use ethers::providers::{Provider, Middleware, Http};
use ethers::types::Address;
use std::sync::Arc;
use futures::stream::{self, StreamExt};
use std::sync::atomic::{AtomicUsize, Ordering};
use rayon::prelude::*;
use futures::stream::{FuturesUnordered};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PoolType {
    V2,
    V3,
}

impl Default for PoolType {
    fn default() -> Self {
        PoolType::V2
    }
}

#[derive(Clone, Debug, Default)]
pub struct PoolState {
    pub pool_type: PoolType,
    pub token0: H160,
    pub token1: H160,
    pub reserve0: Option<U256>,        // V2
    pub reserve1: Option<U256>,        // V2
    pub sqrt_price_x96: Option<U256>,  // V3
    pub liquidity: Option<U256>,       // V3
    pub tick: Option<i32>,             // V3
    pub fee: Option<u32>,              // V3
    pub tick_spacing: Option<i32>,     // V3
    pub last_updated: u64,
}

pub type ReserveCache = DashMap<H160, PoolState>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DexType {
    V2,
    V3,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub to: usize,              // index of the destination token
    pub pool_address: H160,     // pool contract address
    pub dex_type: DexType,      // V2 or V3
    pub fee: u32,               // fee in basis points
}

pub type TokenIndex = HashMap<H160, usize>; // token address -> index
pub type IndexToken = Vec<H160>;            // index -> token address
pub type FlatGraph = Vec<Vec<Edge>>;        // adjacency list: token index -> edges

pub type SafeTokenSet = HashSet<H160>;

// Optionally, for richer metadata:
#[derive(Clone, Debug, Default)]
pub struct TokenMeta {
    pub is_safe: bool,
    pub has_transfer_tax: bool,
    // Add more fields as needed
}

/// Helper async function to fetch reserve for a single pair
async fn fetch_reserve(
    pair: PairInfo,
    provider: Arc<Provider<Http>>,
) -> Option<(H160, PoolState)> {
    let address = pair.pair_address;
    let token0 = pair.token0;
    let token1 = pair.token1;
    let now = chrono::Utc::now().timestamp() as u64;
    match pair.dex_version {
        DexVersion::V2 => {
            let contract = UniswapV2Pair::new(address, provider.clone());
            match contract.get_reserves().call().await {
                Ok(res) => {
                    Some((address, PoolState {
                        pool_type: PoolType::V2,
                        token0,
                        token1,
                        reserve0: Some(res.0.into()),
                        reserve1: Some(res.1.into()),
                        sqrt_price_x96: None,
                        liquidity: None,
                        tick: None,
                        fee: None,
                        tick_spacing: None,
                        last_updated: now,
                    }))
                }
                Err(_) => None,
            }
        }
        DexVersion::V3 => {
            let contract = UniswapV3Pool::new(address, provider.clone());
            let slot0_res = contract.slot_0().call().await;
            let liquidity_res = contract.liquidity().call().await;
            match (slot0_res, liquidity_res) {
                (Ok(slot0), Ok(liq)) => {
                    // Use default values for fee and tick_spacing for now
                    let fee = 3000;
                    let tick_spacing = 60;
                    Some((address, PoolState {
                        pool_type: PoolType::V3,
                        token0,
                        token1,
                        reserve0: None,
                        reserve1: None,
                        sqrt_price_x96: Some(slot0.0.into()),
                        liquidity: Some(liq.into()),
                        tick: Some(slot0.1),
                        fee: Some(fee),
                        tick_spacing: Some(tick_spacing),
                        last_updated: now,
                    }))
                }
                _ => None,
            }
        }
    }
}

/// Preload all reserves and state for all pools into the ReserveCache using batching and rayon
pub async fn preload_reserve_cache(
    pairs: &[PairInfo],
    provider: Arc<Provider<Http>>,
    reserve_cache: &Arc<ReserveCache>,
    _max_concurrent: usize,
) {
    let batch_size = 1000;
    let total_pairs = pairs.len();
    let start_time = std::time::Instant::now();
    println!("[CACHE] Starting preload for {} pairs in batches of {}", total_pairs, batch_size);
    let mut success_count = 0;
    let mut error_count = 0;
    let mut v2_loaded = 0;
    let mut v3_loaded = 0;

    for (i, batch) in pairs.chunks(batch_size).enumerate() {
        println!("[CACHE] Processing batch {} ({} pairs)", i + 1, batch.len());
        // 1. Fetch all reserves in parallel (async)
        let mut futs = FuturesUnordered::new();
        for pair in batch.iter().cloned() {
            let provider = provider.clone();
            futs.push(fetch_reserve(pair, provider));
        }
        let mut results = Vec::with_capacity(batch.len());
        while let Some(res) = futs.next().await {
            results.push(res);
        }
        // 2. Process results in parallel (Rayon)
        results.par_iter().for_each(|res| {
            if let Some((address, state)) = res {
                reserve_cache.insert(*address, state.clone());
            }
        });
        // 3. Stats
        let batch_success = results.iter().filter(|x| x.is_some()).count();
        let batch_error = results.len() - batch_success;
        let batch_v2 = results.iter().filter(|x| x.as_ref().map(|(_, s)| s.pool_type == PoolType::V2).unwrap_or(false)).count();
        let batch_v3 = results.iter().filter(|x| x.as_ref().map(|(_, s)| s.pool_type == PoolType::V3).unwrap_or(false)).count();
        success_count += batch_success;
        error_count += batch_error;
        v2_loaded += batch_v2;
        v3_loaded += batch_v3;
        println!("[CACHE][BATCH {}] Success: {}, Errors: {}, V2: {}, V3: {}", i + 1, batch_success, batch_error, batch_v2, batch_v3);
    }
    let duration = start_time.elapsed();
    println!("[CACHE] Preload completed in {:.2?}", duration);
    println!("[CACHE] Success: {}, Errors: {}, Total: {}", success_count, error_count, total_pairs);
    println!("[CACHE] V2 pools: {}, V3 pools: {}", v2_loaded, v3_loaded);
    println!("[CACHE] Average speed: {:.2} pools/sec", total_pairs as f64 / duration.as_secs_f64());
}
