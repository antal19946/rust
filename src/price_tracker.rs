use ethers::prelude::*;
use ethers::types::{H160, H256, Log, I256, U256};
use std::sync::Arc;
use crate::cache::{ReserveCache, PoolType};
use crate::bindings::{UniswapV3Pool};
use futures::StreamExt;
use ethers::abi::{decode, ParamType};
use crate::mempool_decoder::{ArbitrageOpportunity, DecodedSwap};
use crate::token_index::TokenIndexMap;
use crate::route_cache::RoutePath;
use crate::split_route_path::split_route_around_token_x;
use crate::simulate_swap_path::{simulate_buy_path_amounts_array, simulate_sell_path_amounts_array};
use dashmap::DashMap;
use tokio::sync::mpsc;
use rayon::prelude::*;
use std::fs::OpenOptions;
use std::io::Write;
use chrono::{DateTime, Utc, Datelike, Timelike};
use serde_json::json;

/// Start the price tracker: subscribe to V2 Sync and V3 Swap events, update ReserveCache in real time.
pub async fn start_price_tracker(
    ws_provider: Arc<Provider<Ws>>,
    http_provider: Arc<Provider<Http>>,
    reserve_cache: Arc<ReserveCache>,
    token_index: Arc<TokenIndexMap>,
    precomputed_route_cache: Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: mpsc::Sender<ArbitrageOpportunity>,
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
    let v3_swap_topic = H256::from(ethers::utils::keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)"));

    // Deep debug: print topic hash and address info
    println!("[DEBUG] v3_swap_topic = 0x{:x}", v3_swap_topic);
    println!("[DEBUG] v3_addresses.len() = {}", v3_addresses.len());
    // for (i, addr) in v3_addresses.iter().take(5).enumerate() {
    //     println!("[DEBUG] V3 pool address [{}]: {:?}", i, addr);
    // }

    // V2 Sync subscription with arbitrage detection
    let v2_filter = Filter::new()
        .topic0(v2_sync_topic)
        .address(v2_addresses.clone());
    let reserve_cache_v2 = reserve_cache.clone();
    let token_index_v2 = token_index.clone();
    let precomputed_route_cache_v2 = precomputed_route_cache.clone();
    let opportunity_tx_v2 = opportunity_tx.clone();
    let ws_provider_v2 = ws_provider.clone();
    
    tokio::spawn(async move {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 10;
        
        loop {
            match run_v2_monitoring_loop(
                &ws_provider_v2,
                &v2_filter,
                &reserve_cache_v2,
                &token_index_v2,
                &precomputed_route_cache_v2,
                &opportunity_tx_v2
            ).await {
                Ok(_) => {
                    println!("✅ V2 monitoring completed successfully");
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    eprintln!("❌ V2 monitoring error (attempt {}/{}): {}", retry_count, MAX_RETRIES, e);
                    
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
    println!("[DEBUG] Subscribing to V3 Swap logs for {} pools", v3_addresses.len());
    let v3_filter_topic_only = Filter::new().topic0(v3_swap_topic);

    let reserve_cache_v3 = reserve_cache.clone();
    let token_index_v3 = token_index.clone();
    let precomputed_route_cache_v3 = precomputed_route_cache.clone();
    let opportunity_tx_v3 = opportunity_tx.clone();
    let http_provider_v3 = http_provider.clone();
    let ws_provider_v3 = ws_provider.clone();
    
    tokio::spawn(async move {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 10;
        
        loop {
            match run_v3_monitoring_loop(
                &ws_provider_v3,
                &v3_filter_topic_only,
                &reserve_cache_v3,
                &http_provider_v3,
                &token_index_v3,
                &precomputed_route_cache_v3,
                &opportunity_tx_v3
            ).await {
                Ok(_) => {
                    println!("✅ V3 monitoring completed successfully");
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    eprintln!("❌ V3 monitoring error (attempt {}/{}): {}", retry_count, MAX_RETRIES, e);
                    
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
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut retry_count = 0;
    const MAX_RETRIES: u32 = 10;
    
    println!("🔍 DEBUG: V2 monitoring loop starting...");
    
    loop {
        println!("🔍 DEBUG: V2 monitoring session attempt {}/{}", retry_count + 1, MAX_RETRIES);
        match run_single_v2_session(
            ws_provider,
            filter,
            reserve_cache,
            token_index,
            precomputed_route_cache,
            opportunity_tx
        ).await {
            Ok(_) => {
                println!("✅ V2 monitoring session completed successfully");
                break;
            }
            Err(e) => {
                retry_count += 1;
                eprintln!("❌ V2 monitoring error (attempt {}/{}): {}", retry_count, MAX_RETRIES, e);
                
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
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("🔍 DEBUG: Starting single V2 monitoring session...");
    
    // Subscribe to V2 Sync events
    println!("🔍 DEBUG: Subscribing to V2 Sync events...");
    let mut v2_stream = match tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        ws_provider.subscribe_logs(filter)
    ).await {
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
        
        println!("🔍 DEBUG: About to wait for V2 Sync event...");
        
        tokio::select! {
            // Handle V2 Sync events with timeout
            result = tokio::time::timeout(
                tokio::time::Duration::from_secs(10),
                v2_stream.next()
            ) => {
                println!("🔍 DEBUG: V2 Sync timeout result received: {:?}", result.is_ok());
                match result {
                    Ok(Some(log)) => {
                        println!("🔍 DEBUG: Processing V2 Sync event: {:?}", log.address);
                        last_activity = std::time::Instant::now();
                        
                        // Add timeout for event processing
                        match tokio::time::timeout(
                            tokio::time::Duration::from_secs(10),
                            handle_v2_sync_event_with_arbitrage(
                                log, 
                                reserve_cache, 
                                token_index, 
                                precomputed_route_cache,
                                opportunity_tx
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
    http_provider: &Arc<Provider<Http>>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut retry_count = 0;
    const MAX_RETRIES: u32 = 10;
    
    loop {
        match run_single_v3_session(
            ws_provider,
            filter,
            reserve_cache,
            http_provider,
            token_index,
            precomputed_route_cache,
            opportunity_tx
        ).await {
            Ok(_) => {
                println!("✅ V3 monitoring session completed successfully");
                break;
            }
            Err(e) => {
                retry_count += 1;
                eprintln!("❌ V3 monitoring error (attempt {}/{}): {}", retry_count, MAX_RETRIES, e);
                
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
    http_provider: &Arc<Provider<Http>>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
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
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(10),
            v3_stream.next()
        ).await {
            Ok(Some(log)) => {
                last_activity = std::time::Instant::now();
                
                // Add timeout for event processing
                match tokio::time::timeout(
                    tokio::time::Duration::from_secs(10),
                    handle_v3_swap_event_with_arbitrage(
                        log, 
                        reserve_cache, 
                        http_provider,
                        token_index, 
                        precomputed_route_cache,
                        opportunity_tx
                    )
                ).await {
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
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
) -> anyhow::Result<()> {
    // Sync(address indexed pair, uint112 reserve0, uint112 reserve1)
    if log.data.0.len() < 64 {
        anyhow::bail!("Invalid Sync log data");
    }
    let new_reserve0 = U256::from_big_endian(&log.data.0[0..32]);
    let new_reserve1 = U256::from_big_endian(&log.data.0[32..64]);
    let pool = log.address;
    
    // Get old reserves before updating
    let old_reserve0 = reserve_cache.get(&pool).and_then(|s| s.reserve0).unwrap_or(U256::zero());
    let old_reserve1 = reserve_cache.get(&pool).and_then(|s| s.reserve1).unwrap_or(U256::zero());
    
    // Update cache
    if let Some(mut state) = reserve_cache.get_mut(&pool) {
        state.reserve0 = Some(new_reserve0);
        state.reserve1 = Some(new_reserve1);
        state.last_updated = chrono::Utc::now().timestamp() as u64;
    }
    
    // Calculate which token was bought/sold
    let token0_change = new_reserve0.saturating_sub(old_reserve0);
    let token1_change = new_reserve1.saturating_sub(old_reserve1);
    
    // Determine swap direction and amount
    let (token_x, token_x_amount) = if token0_change > U256::zero() {
        // token0 was bought (reserve0 increased)
        if let Some(pool_data) = reserve_cache.get(&pool) {
            (pool_data.token0, token0_change)
        } else {
            return Ok(());
        }
    } else if token1_change > U256::zero() {
        // token1 was bought (reserve1 increased)
        if let Some(pool_data) = reserve_cache.get(&pool) {
            (pool_data.token1, token1_change)
        } else {
            return Ok(());
        }
    } else {
        return Ok(()); // No clear swap direction
    };

    // Create decoded swap for arbitrage detection
    let decoded_swap = DecodedSwap {
        tx_hash: H160::zero(), // Sync events don't have direct tx hash
        pool_address: pool,
        token_x,
        token_x_amount,
        block_number: log.block_number.unwrap_or(U64::zero()).as_u64(),
        timestamp: chrono::Utc::now().timestamp() as u64,
    };

    // Detect arbitrage opportunities
    if let Some(opportunity) = find_arbitrage_opportunity_from_price_tracker(
        &decoded_swap,
        reserve_cache,
        token_index,
        precomputed_route_cache,
    ).await {
        println!("🎯 [Price Tracker] Found arbitrage opportunity! Profit: {}", opportunity.estimated_profit);
        
        // Log the opportunity
        log_opportunity_from_price_tracker(&opportunity);
        
        // Send opportunity for execution
        if let Err(e) = opportunity_tx.send(opportunity).await {
            eprintln!("❌ [Price Tracker] Failed to send arbitrage opportunity: {}", e);
        }
    }

    Ok(())
}

/// Handle a V3 Swap event: decode from log data, update the cache, and detect arbitrage opportunities.
async fn handle_v3_swap_event_with_arbitrage(
    log: Log, 
    reserve_cache: &Arc<ReserveCache>, 
    _http_provider: &Arc<Provider<Http>>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
) -> anyhow::Result<()> {
    if log.data.0.len() != 160 {
        eprintln!("[V3 Swap] Unexpected log data size: {}", log.data.0.len());
        anyhow::bail!("Invalid V3 Swap log size: {}", log.data.0.len());
    }

    let decoded = decode(
        &[
            ParamType::Int(256),     // amount0
            ParamType::Int(256),     // amount1
            ParamType::Uint(160),    // sqrtPriceX96
            ParamType::Uint(128),    // liquidity
            ParamType::Int(24),      // tick
        ],
        &log.data.0,
    )?;

    // let amount0: I256 = decoded[0].clone().into_int().unwrap();
    // let amount1: I256 = decoded[1].clone().into_int().unwrap();
    let sqrt_price_x96 = decoded[2].clone().into_uint().unwrap();
    let liquidity = decoded[3].clone().into_uint().unwrap();
    let tick_token = decoded[4].clone().into_int().unwrap();
    let tick: i32 = I256::from_raw(tick_token).as_i32();

    let pool = log.address;
    
    // Update cache
    if let Some(mut state) = reserve_cache.get_mut(&pool) {
        state.sqrt_price_x96 = Some(sqrt_price_x96);
        state.liquidity = Some(liquidity);
        state.tick = Some(tick);
        state.last_updated = chrono::Utc::now().timestamp() as u64;
    }

    // Determine which token was bought/sold based on amount0/amount1
    // TODO: Fix V3 swap event processing - type issues need to be resolved
    // For now, skip V3 swap event processing to avoid compilation errors
    return Ok(());

    // TODO: V3 swap event processing is temporarily disabled due to type issues
    // Create decoded swap for arbitrage detection
    // let decoded_swap = DecodedSwap {
    //     tx_hash: H160::zero(), // Swap events don't have direct tx hash
    //     pool_address: pool,
    //     token_x,
    //     token_x_amount,
    //     block_number: log.block_number.unwrap_or(U64::zero()).as_u64(),
    //     timestamp: chrono::Utc::now().timestamp() as u64,
    // };

    // Detect arbitrage opportunities
    // if let Some(opportunity) = find_arbitrage_opportunity_from_price_tracker(
    //     &decoded_swap,
    //     reserve_cache,
    //     token_index,
    //     precomputed_route_cache,
    // ).await {
    //     println!("🎯 [Price Tracker] Found arbitrage opportunity! Profit: {}", opportunity.estimated_profit);
    //     
    //     // Log the opportunity
    //     log_opportunity_from_price_tracker(&opportunity);
    //     
    //     // Send opportunity for execution
    //     if let Err(e) = opportunity_tx.send(opportunity).await {
    //         eprintln!("❌ [Price Tracker] Failed to send arbitrage opportunity: {}", e);
    //     }
    // }

    Ok(())
}

/// Find arbitrage opportunities for a decoded swap (price tracker version)
async fn find_arbitrage_opportunity_from_price_tracker(
    decoded_swap: &DecodedSwap,
    reserve_cache: &Arc<ReserveCache>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
) -> Option<ArbitrageOpportunity> {
    // Get token index
    let token_x_index = token_index.address_to_index.get(&decoded_swap.token_x)?;
    let token_x_index_u32 = *token_x_index as u32;
    
    println!("🔍 [Price Tracker] Finding arbitrage for tokenX (idx {}): {}", token_x_index, decoded_swap.token_x);

    // Get all routes that contain this token and the affected pool
    let candidate_routes = precomputed_route_cache
        .get(&token_x_index_u32)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();

    println!("📊 [Price Tracker] Found {} candidate routes for tokenX", candidate_routes.len());

    // Filter routes that contain the affected pool
    let filtered_routes: Vec<&RoutePath> = candidate_routes.iter()
        .filter(|route| route.pools.contains(&decoded_swap.pool_address))
        .collect();

    println!("🎯 [Price Tracker] {} routes contain the affected pool {}", filtered_routes.len(), decoded_swap.pool_address);

    if filtered_routes.is_empty() {
        return None;
    }

    // Simulate all filtered routes in parallel
    let simulation_results: Vec<Option<crate::arbitrage_finder::SimulatedRoute>> = filtered_routes.par_iter()
        .map(|route| {
            // Split route into buy/sell paths
            let (buy_path, sell_path) = split_route_around_token_x(route, token_x_index_u32)?;
            
            // Simulate buy path (base -> tokenX)
            let buy_amounts = simulate_buy_path_amounts_array(
                &buy_path, 
                decoded_swap.token_x_amount, 
                reserve_cache, 
                token_index
            )?;

            // Simulate sell path (tokenX -> base)
            let sell_amounts = simulate_sell_path_amounts_array(
                &sell_path, 
                decoded_swap.token_x_amount, 
                reserve_cache, 
                token_index
            )?;

            // Merge amounts: [buy_amounts..., sell_amounts[1..]]
            let mut merged_amounts = buy_amounts.clone();
            merged_amounts.extend_from_slice(&sell_amounts[1..]);

            // Calculate profit
            if merged_amounts.len() >= 2 {
                let amount_in = merged_amounts[0];
                let amount_out = merged_amounts.last().unwrap();
                let profit = amount_out.saturating_sub(amount_in);

                // Only consider profitable trades
                if profit > U256::zero() {
                    // Merge token indices
                    let mut merged_tokens = buy_path.hops.clone();
                    merged_tokens.extend_from_slice(&sell_path.hops[1..]);

                    // Map to symbols
                    let merged_symbols = merged_tokens.iter()
                        .map(|&idx| token_index_to_symbol_from_price_tracker(idx, token_index))
                        .collect();

                    // Merge pools
                    let mut merged_pools = buy_path.pools.clone();
                    merged_pools.extend_from_slice(&sell_path.pools);

                    return Some(crate::arbitrage_finder::SimulatedRoute {
                        merged_amounts,
                        merged_tokens,
                        merged_symbols,
                        merged_pools,
                        profit,
                        buy_path: buy_path.clone(),
                        sell_path: sell_path.clone(),
                    });
                }
            }

            None
        })
        .collect();

    // Filter out None results
    let profitable_routes: Vec<crate::arbitrage_finder::SimulatedRoute> = simulation_results.into_iter()
        .filter_map(|r| r)
        .collect();

    println!("💰 [Price Tracker] Found {} profitable routes", profitable_routes.len());

    if profitable_routes.is_empty() {
        return None;
    }

    // Find the most profitable route
    let best_route = profitable_routes.iter()
        .max_by_key(|route| route.profit)
        .cloned();

    let estimated_profit = best_route.as_ref().map(|r| r.profit).unwrap_or(U256::zero());

    Some(ArbitrageOpportunity {
        decoded_swap: decoded_swap.clone(),
        profitable_routes,
        best_route,
        estimated_profit,
    })
}

/// Helper to map token index to symbol (price tracker version)
fn token_index_to_symbol_from_price_tracker(idx: u32, token_index: &TokenIndexMap) -> String {
    if let Some(addr) = token_index.index_to_address.get(&(idx as u16)) {
        format!("0x{:x}", addr)
    } else {
        format!("token{}", idx)
    }
}

/// Log profitable arbitrage opportunity to file (price tracker version)
fn log_opportunity_from_price_tracker(opportunity: &ArbitrageOpportunity) {
    let now: DateTime<Utc> = Utc::now();
    let log_file_path = format!("arbitrage_opportunities_price_tracker_{}.log", now.format("%Y%m%d_%H%M%S"));
    
    // Create detailed log entry
    let log_entry = json!({
        "source": "price_tracker",
        "timestamp": now.to_rfc3339(),
        "block_number": opportunity.decoded_swap.block_number,
        "pool_address": format!("0x{:x}", opportunity.decoded_swap.pool_address),
        "token_x": format!("0x{:x}", opportunity.decoded_swap.token_x),
        "token_x_amount": opportunity.decoded_swap.token_x_amount.to_string(),
        "estimated_profit": opportunity.estimated_profit.to_string(),
        "profitable_routes_count": opportunity.profitable_routes.len(),
        "best_route": {
            "merged_amounts": opportunity.best_route.as_ref().map(|r| r.merged_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
            "merged_symbols": opportunity.best_route.as_ref().map(|r| r.merged_symbols.clone()),
            "merged_pools": opportunity.best_route.as_ref().map(|r| r.merged_pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
            "profit": opportunity.best_route.as_ref().map(|r| r.profit.to_string()),
            "buy_path_hops": opportunity.best_route.as_ref().map(|r| r.buy_path.hops.clone()),
            "sell_path_hops": opportunity.best_route.as_ref().map(|r| r.sell_path.hops.clone()),
        }
    });

    // Write to log file
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path) {
        
        if let Err(e) = writeln!(file, "{}", serde_json::to_string_pretty(&log_entry).unwrap()) {
            eprintln!("❌ [Price Tracker] Failed to write to log file: {}", e);
        }
    } else {
        eprintln!("❌ [Price Tracker] Failed to open log file: {}", log_file_path);
    }

    // Also print summary to console
    println!("📝 [Price Tracker] Logged opportunity to: {}", log_file_path);
}
