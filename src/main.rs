mod config;
mod fetch_pairs;
mod cache;
mod bindings;
mod price_tracker;
mod route_cache;
mod best_route_finder;
mod token_index;
mod token_graph;
mod utils;
mod split_route_path;
mod simulate_swap_path;
mod v3_math;
mod arbitrage_finder;
mod executor;
mod token_tax;
// mod ipc_feed;
mod tx_decoder;
// mod revm_sim;
mod ipc_event_listener;
use alloy_provider::{network::Ethereum, DynProvider, ProviderBuilder};
use ethers::abi::token;
use ethers::providers::{Provider, Http, Ws};
use std::sync::Arc;
use config::Config;
use fetch_pairs::{PairFetcher, PairInfo};
use cache::{ReserveCache};
// use ethers::providers::{ Http, Ws};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Duration;
use dashmap::DashMap;
use std::collections::HashMap;
use ethers::types::H160;
use once_cell::sync::Lazy;
use std::sync::RwLock;
use primitive_types::U256;
use std::str::FromStr;
use route_cache::{build_route_cache, PoolMeta, DEXType, RoutePath};
use split_route_path::split_route_around_token_x;
use simulate_swap_path::{simulate_buy_path, simulate_sell_path, simulate_buy_path_amounts_vec, simulate_sell_path_amounts_vec};
// use arbitrage_finder::{simulate_all_paths_for_token_x, print_simulated_route};
use mempool_decoder::{start_mempool_monitoring, MempoolDecoder};
use rayon::prelude::*;
use crate::executor::{BuySellExecutionData, SwapExecutionData, execute_arbitrage_onchain, execute_arbitrage_onchain_legacy, decode_revert_reason};
use std::env;
use ethers::signers::LocalWallet;
use ethers::signers::Signer;
use dotenv::dotenv;
use std::fs::OpenOptions;
use std::io::Write;
use crate::token_tax::{load_token_tax_map, TokenTaxMap};
use alloy_provider::Provider as AlloyProviderTrait;
use tokio::net::UnixStream;
#[tokio::main]
async fn main() {
    dotenv().ok();
    // Start background IPC event listener
    // ipc_event_listener::spawn_ipc_event_listener();
    println!("🚀 Starting Ultra-Low Latency Arbitrage Bot...");
    let config = Config::default();

    // --- Add contract address and wallet initialization ---
    let contract_address = H160::from_str(&env::var("CONTRACT_ADDRESS").expect("CONTRACT_ADDRESS env var not set")).expect("Invalid contract address");
    let wallet: LocalWallet = env::var("PRIVATE_KEY")
        .expect("PRIVATE_KEY env var not set")
        .parse::<LocalWallet>()
        .expect("Invalid private key")
        .with_chain_id(56u64); // BSC mainnet

    // Check if we should fetch pairs from factories
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--fetch-pairs" {
        println!("📡 Fetching pairs from DEX factories...");
        let fetcher = PairFetcher::new(config.clone());
        if let Err(e) = fetcher.fetch_all_pairs().await {
            eprintln!("❌ Error fetching pairs: {}", e);
            return;
        }
        println!("✅ Pair fetching completed! You can now run the bot without --fetch-pairs flag.");
        return;
    }

    // Test V3 math with realistic values and sanity checks
    println!("\n🧪 TESTING V3 MATH FIXES...");
    v3_math::test_v3_math();
    println!("✅ V3 math test completed\n");

    // Test PancakeSwap V2 simulation accuracy
    simulate_swap_path::test_pancakeswap_v2_simulation();
    
    // Test V3 simulation
    simulate_swap_path::test_v3_simulation();
    
    // Test dynamic V2 fee implementation
    simulate_swap_path::test_dynamic_v2_fees();

    // Load pairs from files
    let mut pairs: Vec<PairInfo> = Vec::new();
    let mut v3_count = 0;
    let mut files_found = 0;
    // 
    for file_path in ["data/liquid_pairs_v2_accurate_taxed.jsonl", "data/liquid_pairs_v3_new.jsonl"] {
        if let Ok(file) = File::open(file_path) {
            files_found += 1;
            println!("�� Loading pairs from: {}", file_path);
            let reader = BufReader::new(file);
            let mut line_count = 0;
            let mut parse_errors = 0;
            for line in reader.lines() {
                line_count += 1;
                if let Ok(line) = line {
                    match serde_json::from_str::<PairInfo>(&line) {
                        Ok(pair) => {
                            if pair.dex_version == config::DexVersion::V3 {
                                v3_count += 1;
                            }
                            pairs.push(pair);
                        }
                        Err(e) => {
                            parse_errors += 1;
                            if parse_errors <= 3 {
                                println!("❌ Parse error on line {}: {}", line_count, e);
                                println!("   Line content: {}", &line[..std::cmp::min(100, line.len())]);
                            }
                        }
                    }
                }
            }
            println!("   Loaded {} pairs, {} parse errors from {}", pairs.len(), parse_errors, file_path);
        } else {
            println!("❌ Could not open file: {}", file_path);
        }
    }
    
    // if files_found == 0 {
    //     println!("❌ No pair files found! Please fetch pairs first:");
    //     println!("   cargo run -- --fetch-pairs");
    //     return;
    // }
    
    println!("Loaded {} pairs from files ({} V3 pairs).", pairs.len(), v3_count);

    // --- Preload token tax info ---
    println!("Preloading token tax info...");
    let token_tax_map: Arc<TokenTaxMap> = Arc::new(load_token_tax_map("data/token_zero_transfer_tax.jsonl"));
    println!("Loaded {} tokens with tax info.", token_tax_map.len());

    // Build providers and cache
    let provider = Arc::new(Provider::<Http>::try_from(&config.rpc_url).expect("provider"));
    let ws_provider = Arc::new(Provider::<Ws>::connect(&config.ws_url).await.expect("ws provider"));
    let reserve_cache = Arc::new(ReserveCache::default());
    // Preload reserves in parallel
    println!("Preloading reserves for all pools...");
    cache::preload_reserve_cache(&pairs, provider.clone(), &reserve_cache, 2000).await;
    println!("Reserve cache loaded: {} pools", reserve_cache.len());
    price_tracker::start_price_tracker(
            // provider.clone(),
            ws_provider.clone(),
            reserve_cache.clone(),
            // token_tax_map.clone(),
        ).await.expect("Failed to start price tracker");



    // --- Build fee map: pool address -> fee (bps) ---

    // Build safe_tokens set as H160 (for memory-efficient filtering)

  
    // Start price tracker
    println!("Starting price tracker (WS event listener)...");
    
    // Create channel for arbitrage opportunities from price tracker
    let (price_tracker_tx, mut price_tracker_rx) = tokio::sync::mpsc::channel::<mempool_decoder::ArbitrageOpportunity>(1000);
    
    // We'll start the price tracker after building the token index and route cache
    println!("Price tracker will be started after building caches...");

    // --- Best Route Finder Integration ---
    // use best_route_finder::{populate_best_routes_for_all_tokens, BestRoute};
    // use dashmap::DashMap;
    use token_index::TokenIndexMap;
    // use token_graph::TokenGraph;
    use ethers::types::H160;

    // Build token index and token graph
    let token_index_map = TokenIndexMap::build_from_reserve_cache(&reserve_cache);
    // let token_graph = TokenGraph::build(&reserve_cache, &token_index_map);



    // Load all base tokens from config
        //     let base_tokens: Vec<u32> = config.base_tokens.iter()
        // .filter_map(|bt| token_index_map.address_to_index.get(&bt.address).copied())
        // .collect();
    // println!("[DEBUG] Using base tokens: {:?}", config.base_tokens.iter().map(|bt| format!("{}: {}", bt.symbol, bt.address)).collect::<Vec<_>>());
    // println!("[DEBUG] Base token indices: {:?}", base_tokens);

    // Track all tokens in graph
            // let tracked_tokens: Vec<u32> = token_index_map.index_to_address.keys().cloned().collect();

            // let route_cache: DashMap<u32, BestRoute> = DashMap::new();
    // println!("Populating best routes for all tokens (parallel)...");
    // populate_best_routes_for_all_tokens(
    //     &token_graph,
    //     &reserve_cache,
    //     &token_index_map,
    //     &base_tokens,
    //     &tracked_tokens,
    //     &route_cache,
    // );
    // println!("Best route cache populated: {} tokens", route_cache.len());
    
    // let token_address = H160::from_str("0x6EaDc05928ACd93eFB3FA0DFbC644D96C6Aa1Df8").unwrap();
    // let token_index = token_index_map.address_to_index.get(&token_address);
    
    // if token_index.is_none() {
    //     println!("❌ Token address {} not found in token_index_map", token_address);
    //     println!("Available tokens: {}", token_index_map.address_to_index.len());
    //     if token_index_map.address_to_index.len() > 0 {
    //         println!("First few tokens: {:?}", token_index_map.address_to_index.iter().take(3).collect::<Vec<_>>());
    //     }
    //     return;
    // }
    // let token_index = token_index.unwrap();

    // if let Some(route) = route_cache.get(token_index) {
    //     println!("Best BUY route for USDT: {:#?}", route.best_buy);
    //     println!("Best SELL route for USDT: {:#?}", route.best_sell);
    // } else {
    //     println!("No route found for USDT index {}", token_index);
    // }

    // Build all_pools: Vec<PoolMeta> from pairs
    let all_pools: Vec<PoolMeta> = pairs.iter().map(|pair| {
        let dex_type = match (pair.dex_name.as_str(), pair.dex_version.clone()) {
            ("PancakeSwap V2", config::DexVersion::V2) => DEXType::PancakeV2,
            ("PancakeSwap V3", config::DexVersion::V3) => DEXType::PancakeV3,
            ("dex V3", config::DexVersion::V3) => DEXType::Other("dex V3".to_string()),
            ("BiSwap", config::DexVersion::V2) => DEXType::BiSwapV2,
            ("Uniswap v3", config::DexVersion::V3) => DEXType::BiSwapV3,
            ("ApeSwap", config::DexVersion::V2) => DEXType::ApeSwapV2,
            ("ApeSwap", config::DexVersion::V3) => DEXType::ApeSwapV3,
            ("BakerySwap", config::DexVersion::V2) => DEXType::BakeryV2,
            ("BakerySwap", config::DexVersion::V3) => DEXType::BakeryV3,
            ("MDEX", config::DexVersion::V2) => DEXType::Other("MDEX".to_string()),
            ("SushiSwap BSC", config::DexVersion::V2) => DEXType::SushiV2,
            ("SushiSwap BSC", config::DexVersion::V3) => DEXType::SushiV3,
            (other, _) => DEXType::Other(other.to_string()),
        };
        let (factory, fee) = if pair.dex_version == config::DexVersion::V3 {
            (Some(pair.factory_address), Some(2500u32)) // TODO: Use actual fee if available
        } else {
            (None, None)
        };
        PoolMeta {
            token0: pair.token0,
            token1: pair.token1,
            address: pair.pair_address,
            dex_type,
            factory,
            fee,
        }
    }).collect();

    // Debug print: Show all V3 pools with their factory and fee
    // for pool in &all_pools {
    //     if let Some(factory) = pool.factory {
    //         println!("[DEBUG] V3 Pool: {:?}, Factory: {:?}, Fee: {:?}", pool.address, factory, pool.fee);
    //     }
    // }

    let pool_meta_map: HashMap<H160, PoolMeta> = all_pools.iter().map(|p| (p.address, p.clone())).collect();

    // Build all_tokens: H160 -> u32 (use token_index_map.address_to_index, but as u32)
    let all_tokens: std::collections::HashMap<H160, u32> = token_index_map.address_to_index.iter().map(|(k, v)| (*k, *v as u32)).collect();

    // Build base_tokens as Vec<H160>
    let base_tokens: Vec<H160> = config.base_tokens.iter().map(|bt| bt.address).collect();

    // // --- Precompute token-to-base-token pool mapping for ultra-fast lookup ---
    // use route_cache::build_token_to_base_token_pools;
    // let token_basepools = build_token_to_base_token_pools(&all_pools, &base_tokens);
    // println!("\n[INFO] Precomputed token-to-base-token pool mapping ready! Showing a sample:");
    // // Print a sample: for first 3 tokens, show their base-token pool mapping
    // for (token, base_map) in token_basepools.iter().take(3) {
    //     println!("Token: {:?}", token);
    //     for (base, pools) in base_map.iter().take(3) {
    //         println!("  Base: {:?} => Pools: {:?}", base, pools);
    //     }
    // }
    // println!("[INFO] ... (mapping contains {} tokens)\n", token_basepools.len());
    // let token_addr = "0x4206931337dc273a630d328da6441786bfad668f".parse::<H160>().unwrap();
    // if let Some(base_map) = token_basepools.get(&token_addr) {
    //     println!("All base-token routes for token {:?}:", token_addr);
    //     for (base, pools) in base_map {
    //         println!("  Base: {:?} => Pools: {:?}", base, pools);
    //     }
    // } else {
    //     println!("No routes found for token {:?}", token_addr);
    // }
    // Build the route cache
    let token_tax_info: HashMap<H160, crate::token_tax::TokenTaxInfo> = token_tax_map.iter().map(|entry| (*entry.key(), entry.value().clone())).collect();
    let precomputed_route_cache = build_route_cache(&all_tokens, &all_pools, &base_tokens, &token_tax_info);
    println!("Precomputed route cache built: {} tokens with paths", precomputed_route_cache.len());

    // Print sample for USDT
    // if let Some(usdt) = config.base_tokens.iter().find(|t| t.symbol == "USDT") {
    //     if let Some(usdt_idx) = all_tokens.get(&usdt.address) {
    //         if let Some(paths) = precomputed_route_cache.get(usdt_idx) {
    //             println!("USDT (idx {}) has {} precomputed paths:", usdt_idx, paths.len());
    //             for (i, path) in paths.iter().take(3).enumerate() {
    //                 println!("Path {}: hops={:?} pools={:?} dex_types={:?}", i+1, path.hops, path.pools, path.dex_types);
    //             }
    //         } else {
    //             println!("No precomputed paths for USDT index {}", usdt_idx);
    //         }
    //     }
    // }

    // Print all precomputed paths for a specific token with a pool filter
    // let token_address = H160::from_str("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c").unwrap();
    // let pool_filter = H160::from_str("0x469efaaadb06b5f4618ed1907aba380411f9a200").unwrap(); // Use a pool that exists in routes
    // let token_idx = all_tokens.get(&token_address).copied();
    // if let Some(token_idx) = token_idx {
    //     if let Some(paths) = precomputed_route_cache.get(&token_idx) {
    //         let filtered: Vec<_> = paths.iter()
    //             .enumerate()
                
    //             .collect();
    //         println!(
    //             "Token {} (idx {}) has {} precomputed paths containing pool {}:",
    //             token_address, token_idx, filtered.len(), pool_filter
    //         );
    //         for (i, path) in filtered.iter() {
    //             println!("Path {}: hops={:?} pools={:?} dex_types={:?}", i+1, path.hops, path.pools, path.dex_types);
    //         }
    //         // Split the first filtered path (if any)
    //         if let Some((_, path)) = filtered.first() {
    //             if let Some((buy, sell)) = split_route_around_token_x(path, token_idx) {
    //                 println!("\nSplit for token {} (idx {}):", token_address, token_idx);
    //                 println!("  BUY path:   hops={:?} pools={:?} dex_types={:?}", buy.hops, buy.pools, buy.dex_types);
    //                 println!("  SELL path:  hops={:?} pools={:?} dex_types={:?}", sell.hops, sell.pools, sell.dex_types);

    //                 // Print debug info for BUY pools
    //                 for (i, pool) in buy.pools.iter().enumerate() {
    //                     if let Some(entry) = reserve_cache.get(pool) {
    //                         let e = entry.value();
    //                         println!(
    //                             "BUY Pool {}: type={:?}, reserve0={:?}, reserve1={:?}, sqrt_price_x96={:?}, liquidity={:?}, fee={:?}",
    //                             i, e.pool_type, e.reserve0, e.reserve1, e.sqrt_price_x96, e.liquidity, e.fee
    //                         );
    //                     } else {
    //                         println!("BUY Pool {}: not found in reserve cache", i);
    //                     }
    //                 }
    //                 // Print debug info for SELL pools
    //                 for (i, pool) in sell.pools.iter().enumerate() {
    //                     if let Some(entry) = reserve_cache.get(pool) {
    //                         let e = entry.value();
    //                         println!(
    //                             "SELL Pool {}: type={:?}, reserve0={:?}, reserve1={:?}, sqrt_price_x96={:?}, liquidity={:?}, fee={:?}",
    //                             i, e.pool_type, e.reserve0, e.reserve1, e.sqrt_price_x96, e.liquidity, e.fee
    //                         );
    //                     } else {
    //                         println!("SELL Pool {}: not found in reserve cache", i);
    //                     }
    //                 }

    //                 // Simulate for a sample tokenX amount (e.g., 0.001 * 1e18)
    //                 let token_x_amount = U256::exp10(12); // 0.001 * 1e18 (very small amount)
    //                 if let Some(buy_result) = simulate_buy_path(&buy, token_x_amount, &reserve_cache, &token_index_map) {
    //                     simulate_swap_path::print_path_simulation_details(&buy_result, "BUY PATH");
    //                 } else {
    //                     println!("Could not simulate BUY path for {} tokenX", token_x_amount);
    //                 }
    //                 if let Some(sell_result) = simulate_sell_path(&sell, token_x_amount, &reserve_cache, &token_index_map) {
    //                     simulate_swap_path::print_path_simulation_details(&sell_result, "SELL PATH");
    //                 } else {
    //                     println!("Could not simulate SELL path for {} tokenX", token_x_amount);
    //                 }

    //                 // --- Print amounts_in and amounts_out vectors for each hop ---
    //                 if let Some((amounts_in, amounts_out)) = simulate_buy_path_amounts_vec(&buy, token_x_amount, &reserve_cache, &token_index_map) {
    //                     println!("BUY amounts_in:  {:?}", amounts_in);
    //                     println!("BUY amounts_out: {:?}", amounts_out);
    //                 } else {
    //                     println!("BUY amounts_in/out: simulation failed");
    //                 }
    //                 if let Some((amounts_in, amounts_out)) = simulate_sell_path_amounts_vec(&sell, token_x_amount, &reserve_cache, &token_index_map) {
    //                     println!("SELL amounts_in:  {:?}", amounts_in);
    //                     println!("SELL amounts_out: {:?}", amounts_out);
    //                 } else {
    //                     println!("SELL amounts_in/out: simulation failed");
    //                 }

    //                 // --- Print PancakeSwap Router format (single array) ---
    //                 use simulate_swap_path::{simulate_buy_path_amounts_array, simulate_sell_path_amounts_array};
    //                 if let Some(amounts) = simulate_buy_path_amounts_array(&buy, token_x_amount, &reserve_cache, &token_index_map) {
    //                     println!("BUY Router Format: {:?}", amounts);
    //                 } else {
    //                     println!("BUY Router Format: simulation failed");
    //                 }
    //                 if let Some(amounts) = simulate_sell_path_amounts_array(&sell, token_x_amount, &reserve_cache, &token_index_map) {
    //                     println!("SELL Router Format: {:?}", amounts);
    //                 } else {
    //                     println!("SELL Router Format: simulation failed");
    //                 }

    //                 // --- Test Comprehensive Function ---
    //                 println!("\n=== TESTING COMPREHENSIVE FUNCTION ===");
    //                 use simulate_swap_path::{simulate_all_filtered_routes, print_comprehensive_results};
    //                 if let Some(comprehensive_results) = simulate_all_filtered_routes(
    //                     token_address,
    //                     pool_filter,
    //                     token_x_amount,
    //                     &all_tokens,
    //                     &precomputed_route_cache,
    //                     &reserve_cache,
    //                     &token_index_map,
    //                 ) {
    //                     print_comprehensive_results(&comprehensive_results);
    //                 } else {
    //                     println!("No filtered routes found for token {} and pool {}", token_address, pool_filter);
    //                 }

    //                 // --- Test New Arbitrage Finder Logic ---
    //                 println!("\n=== TESTING NEW ARBITRAGE FINDER LOGIC ===");
    //                 let arbitrage_results = simulate_all_paths_for_token_x(
    //                     token_idx,
    //                     token_x_amount,
    //                     pool_filter,
    //                     &precomputed_route_cache,
    //                     &reserve_cache,
    //                     &token_index_map,
    //                 );
    //                 println!("Found {} arbitrage paths for token {} (idx {}) and pool {}", 
    //                     arbitrage_results.len(), token_address, token_idx, pool_filter);
                    
    //                 // Print top 3 most profitable paths
    //                 let mut sorted_results = arbitrage_results.clone();
    //                 sorted_results.sort_by(|a, b| b.profit.cmp(&a.profit));
                    
    //                 for (i, route) in sorted_results.iter().take(3).enumerate() {
    //                     println!("\n--- Top {} Arbitrage Path ---", i + 1);
    //                     print_simulated_route(route);
    //                     println!("  Buy Path: hops={:?} pools={:?}", route.buy_path.hops, route.buy_path.pools);
    //                     println!("  Sell Path: hops={:?} pools={:?}", route.sell_path.hops, route.sell_path.pools);
    //                 }
                    
    //                 if let Some(best_route) = sorted_results.first() {
    //                     println!("\n🎯 BEST ARBITRAGE OPPORTUNITY:");
    //                     print_simulated_route(best_route);
    //                     println!("  Full Path: {:?}", best_route.merged_amounts);
    //                     println!("  Token Path: {:?}", best_route.merged_symbols);
    //                     println!("  Pool Path: {:?}", best_route.merged_pools);
    //                 } else {
    //                     println!("❌ No profitable arbitrage paths found");
    //                 }
    //             }
    //         }
    //     } else {
    //         println!("No precomputed paths for token index {}", token_idx);
    //     }
    // } else {
    //     println!("Token address not found in all_tokens map");
    // }

    // --- Start Mempool Monitoring for Real-Time Arbitrage ---
    println!("\n🚀 Starting real-time mempool monitoring...");
    
    let token_index_arc = Arc::new(token_index_map);
    let precomputed_route_cache_arc = Arc::new(precomputed_route_cache);
    
    // Remove the old mempool listener and spawn the new IPC feed listener in the background
    // let http_url = "http://127.0.0.1:8545";
    // let ws_url = "ws://127.0.0.1:8546";
    // let dyn_provider: alloy_provider::DynProvider = alloy_provider::ProviderBuilder::new().connect(http_url).await.expect("Failed to connect HTTP provider").erased();
    // let dyn_provider = Arc::new(dyn_provider);
    // let reserve_cache_for_ipc = reserve_cache.clone();
    // let token_index_arc_for_ipc = token_index_arc.clone();
    // let precomputed_route_cache_arc_for_ipc = precomputed_route_cache_arc.clone();
    // let config_for_ipc = config.clone();
    // let opportunity_tx = price_tracker_tx.clone();
    // let token_tax_map_for_ipc = token_tax_map.clone();

    // tokio::spawn(async move {
    //     if let Err(e) = ipc_feed::listen_and_fetch_details(
    //         ws_url,
    //         http_url,
    //         dyn_provider,
    //         &reserve_cache_for_ipc,
    //         &token_index_arc_for_ipc,
    //         &precomputed_route_cache_arc_for_ipc,
    //         &token_tax_map_for_ipc,
    //         &config_for_ipc,
    //         &opportunity_tx
    //     ).await {
    //         eprintln!("[IPC FEED] Error: {e}");
    //     }
    // });

    // Start price tracker now that we have all the required data structures
    ipc_event_listener::test_arb(&reserve_cache, &token_index_arc, &precomputed_route_cache_arc, &token_tax_map, &config).await;
    ipc_event_listener::spawn_ipc_event_listener_with_cache(
        reserve_cache.clone(),
        token_index_arc.clone(),
        precomputed_route_cache_arc.clone(),
        token_tax_map.clone(),
        config.clone(),
        price_tracker_tx.clone(),
    ).await;
   
    
    // Process arbitrage opportunities from both mempool and price tracker
    let mut opportunity_count = 0;
    let mut total_profit = U256::zero();
    
    // Add timeout and heartbeat monitoring
    let mut last_heartbeat = std::time::Instant::now();
    const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes
    
    println!("📡 Listening for arbitrage opportunities in real-time...");
    println!("💡 Press Ctrl+C to stop the bot");
    println!("🔍 DEBUG: Starting main event loop...");
    
    // Process opportunities from both mempool and price tracker with proper error handling
    loop {
        println!("🔍 DEBUG: Loop iteration start - checking heartbeat...");
        
        // Check for heartbeat timeout
        if last_heartbeat.elapsed() > HEARTBEAT_TIMEOUT {
            println!("⚠️ No activity for 5 minutes, checking system health...");
            last_heartbeat = std::time::Instant::now();
        }
        
        // println!("🔍 DEBUG: About to enter tokio::select!...");
        
        tokio::select! {
            // Handle arbitrage opportunities with timeout
            result = tokio::time::timeout(
                tokio::time::Duration::from_secs(30), 
                price_tracker_rx.recv()
            ) => {
                // handle result (merge logic from both previous arms here)
                match result {
                    Ok(Some(opportunity)) => {
                        last_heartbeat = std::time::Instant::now();
                        opportunity_count += 1;
                        total_profit = total_profit.saturating_add(opportunity.estimated_profit);
                        if let Some(best_route) = &opportunity.best_route {
                            println!("\n🏆 BEST ARBITRAGE ROUTE:");
                            if let Some(swap_data) = BuySellExecutionData::from_simulated_route(
                                best_route,
                                &pool_meta_map,
                                &token_index_arc,
                            ) {
                                let contract_address = contract_address;
                                let wallet = wallet.clone();
                                let provider = provider.clone();
                                tokio::spawn(async move {
                                    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open("executor.log") {
                                        let _ = writeln!(file, "[EXECUTOR CALL] contract_address={:?}, swap_data={:?}", contract_address, swap_data);
                                    }
                                    let result = execute_arbitrage_onchain(
                                        contract_address,
                                        swap_data,
                                        wallet,
                                        provider
                                    ).await;
                                    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open("executor.log") {
                                        match &result {
                                            Ok(tx_hash) => { let _ = writeln!(file, "[EXECUTOR RESULT] Success: tx_hash={:?}", tx_hash); },
                                            Err(e) => {
                                                let msg = e.to_string();
                                                let decoded = if let Some(idx) = msg.find("0x08c379a0") {
                                                    let hex_data = &msg[idx..].split_whitespace().next().unwrap_or("");
                                                    decode_revert_reason(hex_data)
                                                } else { None };
                                                if let Some(reason) = decoded {
                                                    let _ = writeln!(file, "[EXECUTOR RESULT] Error: {} | Decoded: {}", msg, reason);
                                                } else {
                                                    let _ = writeln!(file, "[EXECUTOR RESULT] Error: {}", msg);
                                                }
                                            },
                                        }
                                    }
                                    match result {
                                        Ok(tx_hash) => println!("[ARBITRAGE EXECUTED] Tx hash: {tx_hash:?}"),
                                        Err(e) => eprintln!("[ARBITRAGE ERROR] {e}"),
                                    }
                                });
                            } else {
                                eprintln!("Failed to build BuySellExecutionData for best route");
                            }
                        }
                    }
                    Ok(None) => {
                        println!("❌ Price tracker channel closed, stopping bot...");
                        break;
                    }
                    Err(_) => {
                        println!("⏰ Price tracker timeout (normal), continuing...");
                    }
                }
            }
            // Periodic heartbeat to show the bot is alive
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(60)) => {
                println!("💓 Bot heartbeat - {} opportunities found, {} total profit", opportunity_count, total_profit);
                last_heartbeat = std::time::Instant::now();
            }
            // Handle Ctrl+C gracefully
            _ = tokio::signal::ctrl_c() => {
                println!("\n🛑 Received Ctrl+C, shutting down gracefully...");
                break;
            }
        }
        
        println!("🔍 DEBUG: Loop iteration end");
    }

    println!("📊 Final Summary:");
    println!("  Total Opportunities: {}", opportunity_count);
    println!("  Total Estimated Profit: {}", total_profit);
    println!("  Average Profit per Opportunity: {}", 
        if opportunity_count > 0 { total_profit / U256::from(opportunity_count) } else { U256::zero() });
    println!("✅ Bot shutdown complete!");
    
    // Helpful message for users
    println!("\n💡 TIP: To fetch fresh pairs from DEX factories, run:");
    println!("   cargo run -- --fetch-pairs");

    // Example usage of token_basepools
 
}
