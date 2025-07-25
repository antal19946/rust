use crate::bindings::UniswapV3Pool;
use crate::cache::{PoolType, ReserveCache};
use crate::mempool_decoder::{ArbitrageOpportunity, DecodedSwap};
use crate::route_cache::RoutePath;
use crate::config::Config;
use crate::simulate_swap_path::{
    simulate_buy_path_amounts_array, simulate_sell_path_amounts_array,
};
use crate::split_route_path::split_route_around_token_x;
use crate::token_index::TokenIndexMap;
use crate::token_tax::TokenTaxMap;
use chrono::{DateTime, Datelike, Timelike, Utc};
use dashmap::DashMap;
use ethers::abi::{ParamType, decode};
use ethers::prelude::*;
use ethers::types::{H160, H256, I256, Log, U256};
use futures::StreamExt;
use rayon::prelude::*;
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::mpsc;
use std::time::Instant;

/// Start the price tracker: subscribe to V2 Sync and V3 Swap events, update ReserveCache in real time.
pub async fn start_price_tracker(
    ws_provider: Arc<Provider<Ws>>,
    // http_provider: Arc<Provider<Http>>,
    reserve_cache: Arc<ReserveCache>,
    // token_index: Arc<TokenIndexMap>,
    // precomputed_route_cache: Arc<DashMap<u32, Vec<RoutePath>>>,
    // opportunity_tx: mpsc::Sender<ArbitrageOpportunity>,
    // token_tax_map: Arc<TokenTaxMap>,
    // config: Config,
) -> anyhow::Result<()> {
    // Collect all V2 and V3 pool addresses from the cache
    let mut v2_addresses = vec![];
    let mut v3_addresses = vec![];
    for entry in reserve_cache.iter() {
        match entry.value().pool_type {
            PoolType::V2 => v2_addresses.push(*entry.key()),
            PoolType::V3 => v3_addresses.push(*entry.key()),
        }
    }

    // Topics
    let v2_sync_topic = H256::from(ethers::utils::keccak256(b"Sync(uint112,uint112)"));
    let uniswap_v3_swap_topic = H256::from(ethers::utils::keccak256(
        b"Swap(address,address,int256,int256,uint160,uint128,int24)",
    ));
    let pancakeswap_v3_swap_topic = H256::from(ethers::utils::keccak256(
        b"Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)",
    ));

    // Deep debug: print topic hash and address info
    // println!("[DEBUG] v3_swap_topic = 0x{:x}", uniswap_v3_swap_topic);
    // println!("[DEBUG] v3_addresses.len() = {}", v3_addresses.len());
    // for (i, addr) in v3_addresses.iter().take(5).enumerate() {
    //     println!("[DEBUG] V3 pool address [{}]: {:?}", i, addr);
    // }

    // V2 Sync subscription with arbitrage detection
    let v2_filter = Filter::new()
        .topic0(v2_sync_topic)
        .address(v2_addresses.clone());
    let reserve_cache_v2 = reserve_cache.clone();
    // let token_index_v2 = token_index.clone();
    // let precomputed_route_cache_v2 = precomputed_route_cache.clone();
    // let opportunity_tx_v2 = opportunity_tx.clone();
    let ws_provider_v2 = ws_provider.clone();
    // let token_tax_map_v2 = token_tax_map.clone();

    tokio::spawn(async move {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 10;

        loop {
            match run_v2_monitoring_loop(
                &ws_provider_v2,
                &v2_filter,
                &reserve_cache_v2,
                // &token_index_v2,
                // &precomputed_route_cache_v2,
                // &opportunity_tx_v2,
                // &token_tax_map_v2,
                // &config,
            )
            .await
            {
                Ok(_) => {
                    println!("✅ V2 monitoring completed successfully");
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    eprintln!(
                        "❌ V2 monitoring error (attempt {}/{}): {}",
                        retry_count, MAX_RETRIES, e
                    );

                    if retry_count >= MAX_RETRIES {
                        eprintln!("🚨 Max retries reached, stopping V2 monitoring");
                        break;
                    }

                    // Wait before retrying with exponential backoff
                    let wait_time = std::cmp::min(5 * retry_count, 30); // Max 30 seconds
                    println!("⏳ Waiting {} seconds before V2 retry...", wait_time);
                    tokio::time::sleep(tokio::time::Duration::from_secs(wait_time as u64)).await;
                }
            }
        }
    });

    // V3 Swap subscription with arbitrage detection
    // println!(
    //     "[DEBUG] Subscribing to V3 Swap logs for {} pools",
    //     v3_addresses.len()
    // );
    // Subscribe to both Uniswap V3 and PancakeSwap V3 swap topics
    let v3_filter = Filter::new()
        .topic0(vec![uniswap_v3_swap_topic, pancakeswap_v3_swap_topic]);

    let reserve_cache_v3 = reserve_cache.clone();
    // let token_index_v3 = token_index.clone();
    // let precomputed_route_cache_v3 = precomputed_route_cache.clone();
    // let opportunity_tx_v3 = opportunity_tx.clone();
    // let http_provider_v3 = http_provider.clone();
    let ws_provider_v3 = ws_provider.clone();
    // let token_tax_map_v3 = token_tax_map.clone();

    tokio::spawn(async move {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 10;

        loop {
            match run_v3_monitoring_loop(
                &ws_provider_v3,
                &v3_filter,
                &reserve_cache_v3,
                // &http_provider_v3,
                // &token_index_v3,
                // &precomputed_route_cache_v3,
                // &opportunity_tx_v3,
                // &token_tax_map_v3,
            )
            .await
            {
                Ok(_) => {
                    println!("✅ V3 monitoring completed successfully");
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    eprintln!(
                        "❌ V3 monitoring error (attempt {}/{}): {}",
                        retry_count, MAX_RETRIES, e
                    );

                    if retry_count >= MAX_RETRIES {
                        eprintln!("🚨 Max retries reached, stopping V3 monitoring");
                        break;
                    }

                    // Wait before retrying with exponential backoff
                    let wait_time = std::cmp::min(5 * retry_count, 30); // Max 30 seconds
                    println!("⏳ Waiting {} seconds before V3 retry...", wait_time);
                    tokio::time::sleep(tokio::time::Duration::from_secs(wait_time as u64)).await;
                }
            }
        }
    });

    Ok(())
}

/// V2 monitoring loop with error handling and reconnection
async fn run_v2_monitoring_loop(
    ws_provider: &Arc<Provider<Ws>>,
    filter: &Filter,
    reserve_cache: &Arc<ReserveCache>,
    // token_index: &Arc<TokenIndexMap>,
    // precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    // opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
    // token_tax_map: &Arc<TokenTaxMap>,
    // config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut retry_count = 0;
    const MAX_RETRIES: u32 = 10;

    println!("🔍 DEBUG: V2 monitoring loop starting...");

    loop {
        println!(
            "🔍 DEBUG: V2 monitoring session attempt {}/{}",
            retry_count + 1,
            MAX_RETRIES
        );
        match run_single_v2_session(
            ws_provider,
            filter,
            reserve_cache,
            // token_index,
            // precomputed_route_cache,
            // opportunity_tx,
            // token_tax_map,
            // &config,
        )
        .await
        {
            Ok(_) => {
                println!("✅ V2 monitoring session completed successfully");
                break;
            }
            Err(e) => {
                retry_count += 1;
                eprintln!(
                    "❌ V2 monitoring error (attempt {}/{}): {}",
                    retry_count, MAX_RETRIES, e
                );

                if retry_count >= MAX_RETRIES {
                    eprintln!("🚨 Max retries reached, stopping V2 monitoring");
                    return Err(e);
                }

                // Exponential backoff
                let delay = std::time::Duration::from_secs(2_u64.pow(retry_count.min(5)));
                println!("⏳ Retrying in {:?}...", delay);
                tokio::time::sleep(delay).await;
            }
        }
    }

    Ok(())
}

/// Single V2 monitoring session with proper error handling
async fn run_single_v2_session(
    ws_provider: &Arc<Provider<Ws>>,
    filter: &Filter,
    reserve_cache: &Arc<ReserveCache>,
    // token_index: &Arc<TokenIndexMap>,
    // precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    // opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
    // token_tax_map: &Arc<TokenTaxMap>,
    // config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("🔍 DEBUG: Starting single V2 monitoring session...");

    // Subscribe to V2 Sync events
    println!("🔍 DEBUG: Subscribing to V2 Sync events...");
    let mut v2_stream = match tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        ws_provider.subscribe_logs(filter),
    )
    .await
    {
        Ok(Ok(stream)) => {
            println!("🔍 DEBUG: V2 Sync subscription successful");
            stream
        }
        Ok(Err(e)) => {
            eprintln!("❌ Failed to subscribe to V2 Sync events: {}", e);
            return Err(Box::new(e));
        }
        Err(_) => {
            eprintln!("❌ V2 Sync subscription timeout");
            return Err("V2 Sync subscription timeout".into());
        }
    };

    let mut last_activity = std::time::Instant::now();
    const ACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes

    println!("🔍 DEBUG: Starting V2 Sync monitoring loop...");

    // Monitor V2 Sync events with timeout and error handling
    loop {
        // Check for activity timeout
        if last_activity.elapsed() > ACTIVITY_TIMEOUT {
            println!("⚠️ No V2 activity for 5 minutes, restarting session...");
            return Ok(()); // Restart the session
        }

        // println!("🔍 DEBUG: About to wait for V2 Sync event...");

        tokio::select! {
            // Handle V2 Sync events with timeout
            result = tokio::time::timeout(
                tokio::time::Duration::from_secs(10),
                v2_stream.next()
            ) => {
                // println!("🔍 DEBUG: V2 Sync timeout result received: {:?}", result.is_ok());
                match result {
                    Ok(Some(log)) => {
                        // println!("🔍 DEBUG: Processing V2 Sync event: {:?}", log.address);
                        last_activity = std::time::Instant::now();

                        // Add timeout for event processing
                        match tokio::time::timeout(
                            tokio::time::Duration::from_secs(10),
                            handle_v2_sync_event_with_arbitrage(
                                log,
                                reserve_cache,
                                // token_index,
                                // precomputed_route_cache,
                                // opportunity_tx,
                                // token_tax_map,
                                // &config,
                            )
                        ).await {
                            Ok(result) => {
                                if let Err(e) = result {
                                    eprintln!("❌ Error processing V2 Sync event: {}", e);
                                }
                            }
                            Err(_) => {
                                eprintln!("⚠️ V2 Sync event processing timeout, skipping...");
                            }
                        }
                    }
                    Ok(None) => {
                        println!("❌ V2 Sync stream ended");
                        return Ok(()); // Restart the session
                    }
                    Err(_) => {
                        // Timeout - this is normal, just continue
                        println!("⏰ V2 Sync timeout (normal), continuing...");
                    }
                }
            }

            // Periodic activity check
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                println!("💓 V2 heartbeat - last activity: {:?} ago", last_activity.elapsed());
            }
        }
    }
}

/// V3 monitoring loop with error handling and reconnection
async fn run_v3_monitoring_loop(
    ws_provider: &Arc<Provider<Ws>>,
    filter: &Filter,
    reserve_cache: &Arc<ReserveCache>,
    // http_provider: &Arc<Provider<Http>>,
    // token_index: &Arc<TokenIndexMap>,
    // precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    // opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
    // token_tax_map: &Arc<TokenTaxMap>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut retry_count = 0;
    const MAX_RETRIES: u32 = 10;

    loop {
        match run_single_v3_session(
            ws_provider,
            filter,
            reserve_cache,
            // http_provider,
            // token_index,
            // precomputed_route_cache,
            // opportunity_tx,
            // token_tax_map,
        )
        .await
        {
            Ok(_) => {
                println!("✅ V3 monitoring session completed successfully");
                break;
            }
            Err(e) => {
                retry_count += 1;
                eprintln!(
                    "❌ V3 monitoring error (attempt {}/{}): {}",
                    retry_count, MAX_RETRIES, e
                );

                if retry_count >= MAX_RETRIES {
                    eprintln!("🚨 Max retries reached, stopping V3 monitoring");
                    return Err(e);
                }

                // Wait before retrying with exponential backoff
                let wait_time = std::cmp::min(5 * retry_count, 30); // Max 30 seconds
                println!("⏳ Waiting {} seconds before V3 retry...", wait_time);
                tokio::time::sleep(tokio::time::Duration::from_secs(wait_time as u64)).await;
            }
        }
    }

    Ok(())
}

/// Single V3 monitoring session
async fn run_single_v3_session(
    ws_provider: &Arc<Provider<Ws>>,
    filter: &Filter,
    reserve_cache: &Arc<ReserveCache>,
    // http_provider: &Arc<Provider<Http>>,
    // token_index: &Arc<TokenIndexMap>,
    // precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    // opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
    // token_tax_map: &Arc<TokenTaxMap>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut v3_stream = match ws_provider.subscribe_logs(filter).await {
        Ok(stream) => {
            println!("✅ V3 stream initialized successfully");
            stream
        }
        Err(e) => {
            return Err(format!("Failed to subscribe to V3 logs: {}", e).into());
        }
    };

    let mut last_activity = std::time::Instant::now();
    const ACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes

    loop {
        match tokio::time::timeout(tokio::time::Duration::from_secs(10), v3_stream.next()).await {
            Ok(Some(log)) => {
                last_activity = std::time::Instant::now();

                // Add timeout for event processing
                match tokio::time::timeout(
                    tokio::time::Duration::from_secs(10),
                    handle_v3_swap_event_with_arbitrage(
                        log,
                        reserve_cache,
                        // http_provider,
                        // token_index,
                        // precomputed_route_cache,
                        // opportunity_tx,
                        // token_tax_map,
                    ),
                )
                .await
                {
                    Ok(result) => {
                        if let Err(e) = result {
                            eprintln!("[V3 Swap] Error: {}", e);
                        }
                    }
                    Err(_) => {
                        eprintln!("⏰ Timeout processing V3 swap event");
                    }
                }

                // Check for activity timeout
                if last_activity.elapsed() > ACTIVITY_TIMEOUT {
                    println!("⚠️ No V3 activity for 5 minutes, restarting...");
                    return Err("V3 activity timeout".into());
                }
            }
            Ok(None) => {
                println!("❌ V3 stream ended unexpectedly");
                return Err("V3 stream ended".into());
            }
            Err(_) => {
                // Timeout - this is normal, just continue
                println!("⏰ V3 stream timeout (normal), continuing...");
            }
        }
    }

    Err("V3 stream ended unexpectedly".into())
}

/// Handle a V2 Sync event: decode reserves, update the cache, and detect arbitrage opportunities.
async fn handle_v2_sync_event_with_arbitrage(
    log: Log,
    reserve_cache: &Arc<ReserveCache>,
    // token_index: &Arc<TokenIndexMap>,
    // precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    // opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
    // token_tax_map: &Arc<TokenTaxMap>,
    // config: &Config,
) -> anyhow::Result<()> {
    // Sync(address indexed pair, uint112 reserve0, uint112 reserve1)
    if log.data.0.len() < 64 {
        anyhow::bail!("Invalid Sync log data");
    }
    let new_reserve0 = U256::from_big_endian(&log.data.0[0..32]);
    let new_reserve1 = U256::from_big_endian(&log.data.0[32..64]);
    let pool = log.address;

    // Get old reserves before updating
    let old_reserve0 = reserve_cache
        .get(&pool)
        .and_then(|s| s.reserve0)
        .unwrap_or(U256::zero());
    let old_reserve1 = reserve_cache
        .get(&pool)
        .and_then(|s| s.reserve1)
        .unwrap_or(U256::zero());

    // Update cache
    if let Some(mut state) = reserve_cache.get_mut(&pool) {
        state.reserve0 = Some(new_reserve0);
        state.reserve1 = Some(new_reserve1);
        state.last_updated = chrono::Utc::now().timestamp() as u64;
    }
println!("[DEBUG] Updated V2 pool cache for {:?}: reserve0 = {}, reserve1 = {}", pool, new_reserve0, new_reserve1);
    // Calculate which token was bought/sold
    // let token0_change = new_reserve0.saturating_sub(old_reserve0);
    // let token1_change = new_reserve1.saturating_sub(old_reserve1);

    // Determine swap direction and amount
    // let (token_x, token_x_amount) = if new_reserve0 < old_reserve0 {
    //     // token0 bought (reserve0 decreased)
    //     if let Some(pool_data) = reserve_cache.get(&pool) {
    //         (pool_data.token0, old_reserve0.saturating_sub(new_reserve0))
    //     } else {
    //         return Ok(());
    //     }
    // } else if new_reserve1 < old_reserve1 {
    //     // token1 bought (reserve1 decreased)
    //     if let Some(pool_data) = reserve_cache.get(&pool) {
    //         (pool_data.token1, old_reserve1.saturating_sub(new_reserve1))
    //     } else {
    //         return Ok(());
    //     }
    // } else {
    //     return Ok(());
    // };
    

    // // Create decoded swap for arbitrage detection
    // let decoded_swap = DecodedSwap {
    //     tx_hash: H160::zero(), // Sync events don't have direct tx hash
    //     pool_address: pool,
    //     token_x,
    //     token_x_amount,
    //     block_number: log.block_number.unwrap_or(U64::zero()).as_u64(),
    //     timestamp: chrono::Utc::now().timestamp() as u64,
    // };

    // // --- Start latency monitoring ---
    // let t0 = Instant::now();
    // let mut timings = serde_json::Map::new();
    // timings.insert("search_start_us".to_string(), serde_json::json!(0));

    // // --- Opportunity search (simulation/filtering) ---
    // let after_sim;
    // let before_tx;
    // let after_tx;
    // let mut tx_hash_str: Option<String> = None;
    // if let Some((opportunity, latency_ms)) = find_arbitrage_opportunity_from_price_tracker(
    //     &decoded_swap,
    //     reserve_cache,
    //     token_index,
    //     precomputed_route_cache,
    //     token_tax_map,
    //     &config,
    // )
    // .await
    // {
    //     after_sim = t0.elapsed().as_micros();
    //     timings.insert("after_sim_us".to_string(), serde_json::json!(after_sim));

    //     // Log the opportunity
    //     // log_opportunity_from_price_tracker(
    //     //     &opportunity,
    //     //     latency_ms,
    //     //     reserve_cache,
    //     //     old_reserve0,
    //     //     old_reserve1,
    //     // );

    //     // --- Before TX fire ---
    //     before_tx = t0.elapsed().as_micros();
    //     timings.insert("before_tx_us".to_string(), serde_json::json!(before_tx));

    //     // --- Simulate TX fire (mock, replace with actual call if needed) ---
    //     // let tx_hash = execute_arbitrage_onchain(...).await?;
    //     // For now, just simulate delay
    //     // tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    //     // after_tx = t0.elapsed().as_micros();
    //     // timings.insert("after_tx_us".to_string(), serde_json::json!(after_tx));
    //     // timings.insert("tx_hash".to_string(), serde_json::json!(tx_hash.to_string()));

    //     // Send opportunity for execution
    //     if let Err(e) = opportunity_tx.send(opportunity).await {
    //         eprintln!(
    //             "❌ [Price Tracker] Failed to send arbitrage opportunity: {}",
    //             e
    //         );
    //     }
    //     after_tx = t0.elapsed().as_micros();
    //     timings.insert("after_tx_us".to_string(), serde_json::json!(after_tx));
    //     timings.insert("tx_hash".to_string(), serde_json::json!(tx_hash_str));

    //     // --- Total ---
    //     let total = t0.elapsed().as_millis();
    //     timings.insert("total_ms".to_string(), serde_json::json!(total));

    //     // Print and log timings
    //     // println!("[LATENCY] Step timings: {}", serde_json::to_string_pretty(&timings).unwrap());
    //     // Optionally, append to a timings log file
    //     if let Ok(mut file) = OpenOptions::new()
    //         .create(true)
    //         .append(true)
    //         .open("latency_breakdown_price_tracker.log")
    //     {
    //         if let Err(e) = writeln!(file, "{}", serde_json::to_string(&timings).unwrap()) {
    //             eprintln!("❌ [Price Tracker] Failed to write latency log: {}", e);
    //         }
    //     }
    // }

    Ok(())
}

/// Handle a V3 Swap event: decode from log data, update the cache, and detect arbitrage opportunities.
async fn handle_v3_swap_event_with_arbitrage(
    log: Log,
    reserve_cache: &Arc<ReserveCache>,
    // _http_provider: &Arc<Provider<Http>>,
    // _token_index: &Arc<TokenIndexMap>,
    // _precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    // _opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
    // _token_tax_map: &Arc<TokenTaxMap>,
) -> anyhow::Result<()> {
    // println!("[DEBUG] V3 event handler called for pool {:?}", log.address);
    if log.topics.is_empty() {
        eprintln!("[V3 Swap] No topics in log");
        anyhow::bail!("No topics in log");
    }
    let topic0 = log.topics[0];
    let uniswap_v3_swap_topic = H256::from(ethers::utils::keccak256(
        b"Swap(address,address,int256,int256,uint160,uint128,int24)",
    ));
    let pancakeswap_v3_swap_topic = H256::from(ethers::utils::keccak256(
        b"Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)",
    ));
    let (sqrt_price_x96, liquidity, tick) = if topic0 == uniswap_v3_swap_topic {
        if log.data.0.len() != 160 {
            eprintln!("[UniswapV3 Swap] Unexpected log data size: {}", log.data.0.len());
            anyhow::bail!("Invalid UniswapV3 Swap log size: {}", log.data.0.len());
        }
        let decoded = decode(
            &[
                ParamType::Int(256),  // amount0
                ParamType::Int(256),  // amount1
                ParamType::Uint(160), // sqrtPriceX96
                ParamType::Uint(128), // liquidity
                ParamType::Int(24),   // tick
            ],
            &log.data.0,
        )?;
        let sqrt_price_x96 = decoded[2].clone().into_uint().unwrap();
        let liquidity = decoded[3].clone().into_uint().unwrap();
        let tick_token = decoded[4].clone().into_int().unwrap();
        let tick: i32 = I256::from_raw(tick_token).as_i32();
        (sqrt_price_x96, liquidity, tick)
    } else if topic0 == pancakeswap_v3_swap_topic {
        if log.data.0.len() != 224 {
            eprintln!("[PancakeV3 Swap] Unexpected log data size: {}", log.data.0.len());
            anyhow::bail!("Invalid PancakeV3 Swap log size: {}", log.data.0.len());
        }
        let decoded = decode(
            &[
                ParamType::Int(256),  // amount0
                ParamType::Int(256),  // amount1
                ParamType::Uint(160), // sqrtPriceX96
                ParamType::Uint(128), // liquidity
                ParamType::Int(24),   // tick
                ParamType::Uint(128), // protocolFeesToken0
                ParamType::Uint(128), // protocolFeesToken1
            ],
            &log.data.0,
        )?;
        let sqrt_price_x96 = decoded[2].clone().into_uint().unwrap();
        let liquidity = decoded[3].clone().into_uint().unwrap();
        let tick_token = decoded[4].clone().into_int().unwrap();
        let tick: i32 = I256::from_raw(tick_token).as_i32();
        (sqrt_price_x96, liquidity, tick)
    } else {
        eprintln!("[V3 Swap] Unknown topic0: {:?}", topic0);
        return Ok(());
    };
    let pool = log.address;
    if let Some(mut state) = reserve_cache.get_mut(&pool) {
        // println!("[DEBUG] Updating V3 pool cache for {:?}", pool);
        state.sqrt_price_x96 = Some(sqrt_price_x96);
        state.liquidity = Some(liquidity);
        state.tick = Some(tick);
        state.last_updated = chrono::Utc::now().timestamp() as u64;
    }
    println!("[DEBUG] Updated V3 pool cache for {:?}: sqrt_price_x96 = {}, liquidity = {}, tick = {}", pool, sqrt_price_x96, liquidity, tick);
    Ok(())
}

/// Find arbitrage opportunities for a decoded swap (price tracker version)
pub async fn find_arbitrage_opportunity_from_price_tracker(
    decoded_swap: &DecodedSwap,
    reserve_cache: &Arc<ReserveCache>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<(ArbitrageOpportunity, u128)> {
    // Start latency timer
    let start_time = std::time::Instant::now();
    // Get token index
    let token_x_index = token_index.address_to_index.get(&decoded_swap.token_x)?;
    let token_x_index_u32 = *token_x_index as u32;

    // println!(
    //     "🔍 [Price Tracker] Finding arbitrage for tokenX (idx {}): {}",
    //     token_x_index, decoded_swap.token_x
    // );

    // Get all routes that contain this token and the affected pool
    let candidate_routes = precomputed_route_cache
        .get(&token_x_index_u32)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();

    // println!(
    //     "📊 [Price Tracker] Found {} candidate routes for tokenX",
    //     candidate_routes.len()
    // );

    // Filter routes that contain the affected pool
    let filtered_routes: Vec<&RoutePath> = candidate_routes
        .iter()
        .filter(|route| route.pools.contains(&decoded_swap.pool_address))
        .collect();

    // println!(
    //     "🎯 [Price Tracker] {} routes contain the affected pool {}",
    //     filtered_routes.len(),
    //     decoded_swap.pool_address
    // );

    if filtered_routes.is_empty() {
        return None;
    }

    // Simulate all filtered routes in parallel
    let simulation_results: Vec<Option<crate::arbitrage_finder::SimulatedRoute>> = filtered_routes
        .par_iter()
        .map(|route| {
            // Split route into buy/sell paths
            let (buy_path, sell_path) = split_route_around_token_x(route, token_x_index_u32)?;

            // Simulate buy path (base -> tokenX)
            let buy_amounts = simulate_buy_path_amounts_array(
                &buy_path,
                decoded_swap.token_x_amount,
                reserve_cache,
                token_index,
                token_tax_map,
                config,
            )?;

            // Simulate sell path (tokenX -> base)
            let sell_amounts = simulate_sell_path_amounts_array(
                &sell_path,
                decoded_swap.token_x_amount,
                reserve_cache,
                token_index,
                token_tax_map,
                config,
            )?;

            // Merge amounts: [buy_amounts..., sell_amounts[1..]]
            let mut merged_amounts = buy_amounts.clone();
            merged_amounts.extend_from_slice(&sell_amounts[1..]);
            // let sell_test_amounts;
            // simulate_sell_path_amounts_array(
            //     route,
            //     merged_amounts[0],
            //     reserve_cache,
            //     token_index,
            // )?;
            // Calculate profit and profit percentage
            if merged_amounts.len() >= 2 {
                let amount_in = merged_amounts[0];
                let amount_out = merged_amounts.last().unwrap();
                let profit = amount_out.saturating_sub(amount_in);

                // Only consider profitable trades
                let sell_symbols: Vec<String> = sell_path
                    .hops
                    .iter()
                    .map(|&idx| token_index_to_symbol_from_price_tracker(idx, token_index))
                    .collect();
                let price_usd = {
                    let last_symbol = &sell_symbols[sell_symbols.len()-1];
                    if let Ok(addr) = last_symbol.parse::<H160>() {
                        get_token_usd_value(&addr).unwrap_or(0.0)
                    } else {
                        0.0
                    }
                };
                let amount = u256_to_f64_lossy(&profit) / 10_f64.powi(18 as i32);
                let profit_usd = amount * price_usd;
                if profit_usd > 0.02 {
                    // Calculate profit percentage (profit / amount_in * 100)
                    let profit_percentage = if amount_in > U256::zero() {
                        // Convert to f64 for percentage calculation
                        let profit_f64 = profit.as_u128() as f64;
                        let amount_in_f64 = amount_in.as_u128() as f64;
                        (profit_f64 / amount_in_f64) * 100.0
                    } else {
                        0.0
                    };

                    // Merge token indices
                    // let mut merged_tokens = buy_path.hops.clone();
                    // merged_tokens.extend_from_slice(&sell_path.hops[1..]);

                    // Map to symbols
                    // let merged_symbols = merged_tokens
                    //     .iter()
                    //     .map(|&idx| token_index_to_symbol_from_price_tracker(idx, token_index))
                    //     .collect();

                    // Merge pools
                    let mut merged_pools = buy_path.pools.clone();
                    merged_pools.extend_from_slice(&sell_path.pools);

                    return Some(crate::arbitrage_finder::SimulatedRoute {
                        merged_amounts,
                        buy_amounts,
                        sell_amounts,
                        // merged_tokens,
                        // merged_symbols,
                        buy_symbols: buy_path
                            .hops
                            .iter()
                            .map(|&idx| token_index_to_symbol_from_price_tracker(idx, token_index))
                            .collect(),
                        sell_symbols,
                            buy_pools: buy_path.pools.clone(),
                        sell_pools: sell_path.pools.clone(),
                        merged_pools,
                        profit,
                        profit_percentage,
                        buy_path: buy_path.clone(),
                        sell_path: sell_path.clone(),
                        // sell_test_amounts,
                    });
                }
            }

            None
        })
        .collect();

    // Filter out None results
    let profitable_routes: Vec<crate::arbitrage_finder::SimulatedRoute> =
        simulation_results.into_iter().filter_map(|r| r).collect();

    // println!(
    //     "💰 [Price Tracker] Found {} profitable routes",
    //     profitable_routes.len()
    // );

    if profitable_routes.is_empty() {
        return None;
    }

    // Find the most profitable route by percentage (better for multiple base tokens)
    let best_route = profitable_routes
        .iter()
        .max_by(|a, b| a.profit_percentage.partial_cmp(&b.profit_percentage).unwrap_or(std::cmp::Ordering::Equal))
        .cloned();

    let estimated_profit = best_route
        .as_ref()
        .map(|r| r.profit)
        .unwrap_or(U256::zero());

    // End latency timer
    let latency = start_time.elapsed().as_millis();

    Some((
        ArbitrageOpportunity {
            decoded_swap: decoded_swap.clone(),
            profitable_routes,
            best_route,
            estimated_profit,
        },
        latency,
    ))
}
fn u256_to_f64_lossy(val: &U256) -> f64 {
    if val.bits() <= 128 {
        val.as_u128() as f64
    } else {
        val.to_string().parse::<f64>().unwrap_or(f64::MAX)
    }
}
const KNOWN_TOKENS: &[(&str, &str, f64)] = &[
    ("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", "BNB", 689.93),
    ("0x2170Ed0880ac9A755fd29B2688956BD959F933F8", "ETH", 2961.19),
    ("0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c", "BTC", 117970.0),
    ("0x55d398326f99059fF775485246999027B3197955", "USDT", 1.00),
    ("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d", "USDC", 1.00), // Multichain bridge price
    ("0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56", "BUSD", 1.00),
    ("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82", "CAKE", 2.37),
];

fn get_token_usd_value(token_address: &H160) -> Option<f64> {
    let addr_str = format!("0x{:x}", token_address);
    KNOWN_TOKENS.iter()
        .find(|(addr, _, _)| addr.to_lowercase() == addr_str.to_lowercase())
        .map(|(_, _, price)| *price)
}
/// Helper to map token index to symbol (price tracker version)
fn token_index_to_symbol_from_price_tracker(idx: u32, token_index: &TokenIndexMap) -> String {
    if let Some(addr) = token_index.index_to_address.get(&(idx as u32)) {
        format!("0x{:x}", addr)
    } else {
        format!("token{}", idx)
    }
}

// fn log_opportunity_from_price_tracker(
//     opportunity: &ArbitrageOpportunity,
//     latency_ms: u128,
//     reserve_cache: &crate::cache::ReserveCache,
//     old_reserve0: U256,
//     old_reserve1: U256,
// ) {
//     use crate::cache::PoolState;
//     use ethers::types::U256;
//     // Fetch current pool state from reserve_cache
//     let (reserve0, reserve1, sqrt_price_x96, liquidity, tick, fee): (
//         Option<U256>,
//         Option<U256>,
//         Option<U256>,
//         Option<U256>,
//         Option<i32>,
//         Option<u32>,
//     ) = {
//         if let Some(state) = reserve_cache.get(&opportunity.decoded_swap.pool_address) {
//             (
//                 state.reserve0,
//                 state.reserve1,
//                 state.sqrt_price_x96,
//                 state.liquidity,
//                 state.tick,
//                 state.fee,
//             )
//         } else {
//             (None, None, None, None, None, None)
//         }
//     };

//     let now: DateTime<Utc> = Utc::now();
//     let log_file_path = format!(
//         "logs/arbitrage_opportunities_price_tracker_{}.log",
//         now.format("%Y%m%d_%H%M%S")
//     );

//     // Create detailed log entry
//     let mut log_entry = json!({
//         "source": "price_tracker",
//         "timestamp": now.to_rfc3339(),
//         "block_number": opportunity.decoded_swap.block_number,
//         "pool_address": format!("0x{:x}", opportunity.decoded_swap.pool_address),
//         "token_x": format!("0x{:x}", opportunity.decoded_swap.token_x),
//         "token_x_amount": opportunity.decoded_swap.token_x_amount.to_string(),
//         "estimated_profit": opportunity.estimated_profit.to_string(),
//         "profitable_routes_count": opportunity.profitable_routes.len(),
//         "latency_ms": latency_ms,
//         "reserve0": reserve0.map(|v| v.to_string()),
//         "reserve1": reserve1.map(|v| v.to_string()),
//         "old_reserve0": old_reserve0.to_string(),
//         "old_reserve1": old_reserve1.to_string(),
//         "sqrt_price_x96": sqrt_price_x96.map(|v| v.to_string()),
//         "liquidity": liquidity.map(|v| v.to_string()),
//         "tick": tick,
//         "best_route": {
//             "merged_amounts": opportunity.best_route.as_ref().map(|r| r.merged_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
//             "merged_symbols": opportunity.best_route.as_ref().map(|r| r.merged_symbols.clone()),
//             "merged_pools": opportunity.best_route.as_ref().map(|r| r.merged_pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
//             "profit": opportunity.best_route.as_ref().map(|r| r.profit.to_string()),
//             "buy_path_hops": opportunity.best_route.as_ref().map(|r| r.buy_path.hops.clone()),
//             "sell_path_hops": opportunity.best_route.as_ref().map(|r| r.sell_path.hops.clone()),
//             "buy_amounts": opportunity.best_route.as_ref().map(|r| r.buy_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
//             "sell_amounts": opportunity.best_route.as_ref().map(|r| r.sell_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
//             "profit_percentage": opportunity.best_route.as_ref().map(|r| r.profit_percentage),
//             "buy_path_pools": opportunity.best_route.as_ref().map(|r| r.buy_path.pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
//             "sell_path_pools": opportunity.best_route.as_ref().map(|r| r.sell_path.pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
//             "buy_symbols": opportunity.best_route.as_ref().map(|r| r.buy_symbols.clone()),
//             "sell_symbols": opportunity.best_route.as_ref().map(|r| r.sell_symbols.clone()),
//             "buy_pools": opportunity.best_route.as_ref().map(|r| r.buy_pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
//             "sell_pools": opportunity.best_route.as_ref().map(|r| r.sell_pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
//             // "sell_test_amounts": opportunity.best_route.as_ref().map(|r| r.sell_test_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),

//         }
//     });

//     // Add pool-wise data as a separate field
//     if let Some(best_route) = &opportunity.best_route {
//         let mut pools_data = serde_json::Map::new();
//         for pool_address in &best_route.merged_pools {
//             let pool_key = format!("0x{:x}", pool_address);
//             let mut pool_info = serde_json::Map::new();
            
//             if let Some(state) = reserve_cache.get(pool_address) {
//                 pool_info.insert("reserve0".to_string(), 
//                     state.reserve0.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
//                 pool_info.insert("reserve1".to_string(), 
//                     state.reserve1.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
//                 pool_info.insert("sqrt_price_x96".to_string(), 
//                     state.sqrt_price_x96.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
//                 pool_info.insert("liquidity".to_string(), 
//                     state.liquidity.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
//                 pool_info.insert("tick".to_string(), 
//                     state.tick.map(|v| serde_json::Value::Number(serde_json::Number::from(v))).unwrap_or(serde_json::Value::Null));
//                 pool_info.insert("fee".to_string(),
//                     state.fee.map(|v| serde_json::Value::Number(serde_json::Number::from(v))).unwrap_or(serde_json::Value::Null));
//                 pool_info.insert("last_updated".to_string(), 
//                     serde_json::Value::Number(serde_json::Number::from(state.last_updated)));
//                 pool_info.insert("pool_type".to_string(), 
//                     serde_json::Value::String(format!("{:?}", state.pool_type)));
//             } else {
//                 pool_info.insert("reserve0".to_string(), serde_json::Value::Null);
//                 pool_info.insert("reserve1".to_string(), serde_json::Value::Null);
//                 pool_info.insert("sqrt_price_x96".to_string(), serde_json::Value::Null);
//                 pool_info.insert("liquidity".to_string(), serde_json::Value::Null);
//                 pool_info.insert("tick".to_string(), serde_json::Value::Null);
//                 pool_info.insert("fee".to_string(), serde_json::Value::Null);
//                 pool_info.insert("last_updated".to_string(), serde_json::Value::Null);
//                 pool_info.insert("pool_type".to_string(), serde_json::Value::String("Unknown".to_string()));
//             }
            
//             pools_data.insert(pool_key, serde_json::Value::Object(pool_info));
//         }
        
//         log_entry.as_object_mut().unwrap().insert("merged_pools_data".to_string(), serde_json::Value::Object(pools_data));
//     }

//     // Write to log file
//     if let Ok(mut file) = OpenOptions::new()
//         .create(true)
//         .append(true)
//         .open(&log_file_path)
//     {
//         if let Err(e) = writeln!(
//             file,
//             "{}",
//             serde_json::to_string_pretty(&log_entry).unwrap()
//         ) {
//             eprintln!("❌ [Price Tracker] Failed to write to log file: {}", e);
//         }
//     } else {
//         eprintln!(
//             "❌ [Price Tracker] Failed to open log file: {}",
//             log_file_path
//         );
//     }

//     // Also print summary to console
//     // println!(
//     //     "📝 [Price Tracker] Logged opportunity to: {} (latency: {} ms)",
//     //     log_file_path, latency_ms
//     // );
// }
