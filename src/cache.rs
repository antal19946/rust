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

/// Preload all reserves and state for all pools into the ReserveCache.
pub async fn preload_reserve_cache(
    pairs: &[PairInfo],
    provider: Arc<Provider<Http>>,
    reserve_cache: &Arc<ReserveCache>,
    max_concurrent: usize,
) {
    static SUCCESS_COUNT: AtomicUsize = AtomicUsize::new(0);
    static ERROR_COUNT: AtomicUsize = AtomicUsize::new(0);

    stream::iter(pairs.iter().cloned())
        .for_each_concurrent(max_concurrent, |pair| {
            let provider = provider.clone();
            let reserve_cache = reserve_cache.clone();
            async move {
                let address = pair.pair_address;
                let token0 = pair.token0;
                let token1 = pair.token1;
                let now = chrono::Utc::now().timestamp() as u64;
                match pair.dex_version {
                    DexVersion::V2 => {
                        let contract = UniswapV2Pair::new(address, provider.clone());
                        match contract.get_reserves().call().await {
                            Ok(res) => {
                                let n = SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
                                if n < 5 {
                                    println!("[V2] Loaded reserves for {:?}: ({}, {})", address, res.0, res.1);
                                }
                                reserve_cache.insert(address, PoolState {
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
                                });
                            }
                            Err(e) => {
                                let n = ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
                                if n < 5 {
                                    eprintln!("[V2] Error loading reserves for {:?}: {}", address, e);
                                }
                            }
                        }
                    }
                    DexVersion::V3 => {
                        let contract = UniswapV3Pool::new(address, provider.clone());
                        let slot0_res = contract.slot_0().call().await;
                        let liquidity_res = contract.liquidity().call().await;
                        let spacing_res = contract.tick_spacing().call().await;
                        let fee_res = contract.fee().call().await;
                        println!("[V3][RESERVE][DEBUG] Pool {:?} (token0={:?}, token1={:?}) slot0_res={:?} liquidity_res={:?} tick_spacing={:?} fee={:?}", address, token0, token1, slot0_res, liquidity_res, spacing_res, fee_res);
                        match (&slot0_res, &liquidity_res) {
                            (Ok(slot0), Ok(liq)) => {
                                let n = SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
                                if n < 5 {
                                    println!("[V3] Loaded slot0/liquidity for {:?}: sqrtPriceX96={}, tick={}, liquidity={}, tick_spacing={:?}, fee={:?}", address, slot0.0, slot0.1, liq, spacing_res, fee_res);
                                }
                                reserve_cache.insert(address, PoolState {
                                    pool_type: PoolType::V3,
                                    token0,
                                    token1,
                                    reserve0: None,
                                    reserve1: None,
                                    sqrt_price_x96: Some(slot0.0.into()),
                                    liquidity: Some((*liq).into()),
                                    tick: Some(slot0.1),
                                    fee: fee_res.ok().map(|x| x as u32),
                                    tick_spacing: spacing_res.ok().map(|x| x as i32),
                                    last_updated: now,
                                });
                            }
                            _ => {
                                let n = ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
                                eprintln!("[V3][ERROR] Pool {:?} (token0={:?}, token1={:?}) slot0_res={:?} liquidity_res={:?}", address, token0, token1, slot0_res, liquidity_res);
                            }
                        }
                    }
                }
            }
        })
        .await;
    println!("[DEBUG] Total successful loads: {}", SUCCESS_COUNT.load(Ordering::Relaxed));
    println!("[DEBUG] Total errors: {}", ERROR_COUNT.load(Ordering::Relaxed));
    let v3_loaded = reserve_cache.iter().filter(|e| e.value().pool_type == PoolType::V3).count();
    println!("[DEBUG] V3 pools loaded in cache: {}", v3_loaded);
}
