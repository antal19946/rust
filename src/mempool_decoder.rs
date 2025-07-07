use crate::arbitrage_finder::{simulate_all_paths_for_token_x, SimulatedRoute};
use crate::cache::ReserveCache;
use crate::route_cache::RoutePath;
use crate::split_route_path::split_route_around_token_x;
use crate::simulate_swap_path::{simulate_buy_path_amounts_array, simulate_sell_path_amounts_array};
use crate::token_index::TokenIndexMap;
use crate::config::Config;
use dashmap::DashMap;
use ethers::{
    providers::{Provider, Ws, Middleware},
    types::{H160, H256, U256, Transaction, Log, U64},
    core::types::Filter,
};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use chrono::{DateTime, Utc, Datelike, Timelike};
use serde_json::json;

/// Mempool transaction with decoded swap information
#[derive(Debug, Clone)]
pub struct DecodedSwap {
    pub tx_hash: H160,
    pub pool_address: H160,
    pub token_x: H160,
    pub token_x_amount: U256,
    pub block_number: u64,
    pub timestamp: u64,
}

/// Arbitrage opportunity detected from mempool
#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub decoded_swap: DecodedSwap,
    pub profitable_routes: Vec<SimulatedRoute>,
    pub best_route: Option<SimulatedRoute>,
    pub estimated_profit: U256,
}

/// Mempool decoder that monitors transactions and detects arbitrage opportunities
pub struct MempoolDecoder {
    provider: Arc<Provider<Ws>>,
    reserve_cache: Arc<ReserveCache>,
    token_index: Arc<TokenIndexMap>,
    precomputed_route_cache: Arc<DashMap<u32, Vec<RoutePath>>>,
    config: Config,
    opportunity_tx: mpsc::Sender<ArbitrageOpportunity>,
    monitored_pools: Vec<H160>, // All pool addresses from reserve_cache
    log_file_path: String,
}

impl MempoolDecoder {
    pub fn new(
        provider: Arc<Provider<Ws>>,
        reserve_cache: Arc<ReserveCache>,
        token_index: Arc<TokenIndexMap>,
        precomputed_route_cache: Arc<DashMap<u32, Vec<RoutePath>>>,
        config: Config,
        opportunity_tx: mpsc::Sender<ArbitrageOpportunity>,
    ) -> Self {
        // Extract all pool addresses from reserve_cache
        let monitored_pools: Vec<H160> = reserve_cache.iter()
            .map(|entry| *entry.key())
            .collect();

        println!("📊 Monitoring {} pools for swap events", monitored_pools.len());

        // Create log file path with timestamp
        let now: DateTime<Utc> = Utc::now();
        let log_file_path = format!("arbitrage_opportunities_{}.log", now.format("%Y%m%d_%H%M%S"));

        Self {
            provider,
            reserve_cache,
            token_index,
            precomputed_route_cache,
            config,
            opportunity_tx,
            monitored_pools,
            log_file_path,
        }
    }

    /// Start monitoring mempool for arbitrage opportunities
    pub async fn start_monitoring(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🚀 Starting mempool monitoring for {} pools...", self.monitored_pools.len());
        
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 10;
        
        loop {
            match self.run_monitoring_loop().await {
                Ok(_) => {
                    println!("✅ Mempool monitoring completed successfully");
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    eprintln!("❌ Mempool monitoring error (attempt {}/{}): {}", retry_count, MAX_RETRIES, e);
                    
                    if retry_count >= MAX_RETRIES {
                        eprintln!("🚨 Max retries reached, stopping mempool monitoring");
                        break;
                    }
                    
                    // Wait before retrying with exponential backoff
                    let wait_time = std::cmp::min(5 * retry_count, 30); // Max 30 seconds
                    println!("⏳ Waiting {} seconds before retry...", wait_time);
                    tokio::time::sleep(tokio::time::Duration::from_secs(wait_time as u64)).await;
                }
            }
        }
        
        Ok(())
    }

    /// Main monitoring loop with proper error handling
    async fn run_monitoring_loop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 10;
        
        println!("🔍 DEBUG: Mempool monitoring loop starting...");
        
        loop {
            println!("🔍 DEBUG: Mempool monitoring session attempt {}/{}", retry_count + 1, MAX_RETRIES);
            match self.run_single_monitoring_session().await {
                Ok(_) => {
                    println!("✅ Mempool monitoring session completed successfully");
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    eprintln!("❌ Mempool monitoring error (attempt {}/{}): {}", retry_count, MAX_RETRIES, e);
                    
                    if retry_count >= MAX_RETRIES {
                        eprintln!("🚨 Max retries reached, stopping mempool monitoring");
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

    /// Run a single monitoring session with proper error handling
    async fn run_single_monitoring_session(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🔍 DEBUG: Starting single mempool monitoring session...");
        
        // Use existing provider instead of creating new one
        println!("🔍 DEBUG: Using existing WebSocket provider...");
        
        // Subscribe to pending transactions
        println!("🔍 DEBUG: Subscribing to pending transactions...");
        let mut pending_stream = match tokio::time::timeout(
            tokio::time::Duration::from_secs(10),
            self.provider.subscribe_pending_txs()
        ).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                eprintln!("❌ Failed to subscribe to pending transactions: {}", e);
                return Err(Box::new(e));
            }
            Err(_) => {
                eprintln!("❌ Pending transaction subscription timeout");
                return Err("Pending transaction subscription timeout".into());
            }
        };
        println!("🔍 DEBUG: Pending transaction subscription successful");
        
        let mut last_activity = std::time::Instant::now();
        const ACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes
        
        println!("🔍 DEBUG: Starting pending transaction monitoring loop...");
        
        // Monitor pending transactions with timeout and error handling
        loop {
            // Check for activity timeout
            if last_activity.elapsed() > ACTIVITY_TIMEOUT {
                println!("⚠️ No mempool activity for 5 minutes, restarting session...");
                return Ok(()); // Restart the session
            }
            
            println!("🔍 DEBUG: About to wait for pending transaction...");
            
            tokio::select! {
                // Handle pending transactions with timeout
                result = tokio::time::timeout(
                    tokio::time::Duration::from_secs(10),
                    pending_stream.next()
                ) => {
                    println!("🔍 DEBUG: Pending transaction timeout result received: {:?}", result.is_ok());
                    match result {
                        Ok(Some(tx_hash)) => {
                            println!("🔍 DEBUG: Processing pending transaction: {:?}", tx_hash);
                            last_activity = std::time::Instant::now();
                            
                            // Add timeout for transaction processing
                            match tokio::time::timeout(
                                tokio::time::Duration::from_secs(10),
                                self.process_pending_transaction(tx_hash)
                            ).await {
                                Ok(result) => {
                                    if let Err(e) = result {
                                        eprintln!("❌ Error processing pending transaction: {}", e);
                                    }
                                }
                                Err(_) => {
                                    eprintln!("⚠️ Transaction processing timeout, skipping...");
                                }
                            }
                        }
                        Ok(None) => {
                            println!("❌ Pending transaction stream ended");
                            return Ok(()); // Restart the session
                        }
                        Err(_) => {
                            // Timeout - this is normal, just continue
                            println!("⏰ Pending transaction timeout (normal), continuing...");
                        }
                    }
                }
                
                // Periodic activity check
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                    println!("💓 Mempool heartbeat - last activity: {:?} ago", last_activity.elapsed());
                }
            }
        }
    }

    /// Process a pending transaction with error handling
    async fn process_pending_transaction(&self, tx_hash: H256) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self.provider.get_transaction(tx_hash).await {
            Ok(Some(tx)) => {
                if let Some(decoded_swap) = self.decode_pool_swap_transaction(&tx).await {
                    println!("📡 Detected swap TX: {} tokenX from pool {}", 
                        decoded_swap.token_x_amount, decoded_swap.pool_address);
                    
                    // Find arbitrage opportunities for this swap
                    if let Some(opportunity) = self.find_arbitrage_opportunity(&decoded_swap).await {
                        println!("🎯 Found arbitrage opportunity! Profit: {}", opportunity.estimated_profit);
                        
                        // Log the opportunity to file
                        self.log_opportunity(&opportunity);
                        
                        // Send opportunity for execution
                        if let Err(e) = self.opportunity_tx.send(opportunity).await {
                            eprintln!("❌ Failed to send arbitrage opportunity: {}", e);
                        }
                    }
                }
            }
            Ok(None) => {
                // Transaction not found, this is normal
            }
            Err(e) => {
                eprintln!("❌ Error fetching transaction {}: {}", tx_hash, e);
            }
        }
        Ok(())
    }

    /// Process a sync event with error handling
    async fn process_sync_event(&self, log: Log) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(decoded_swap) = self.decode_sync_event(&log).await {
            println!("📡 Detected Sync event: {} tokenX from pool {}", 
                decoded_swap.token_x_amount, decoded_swap.pool_address);
            
            // Find arbitrage opportunities for this sync
            if let Some(opportunity) = self.find_arbitrage_opportunity(&decoded_swap).await {
                println!("🎯 Found arbitrage opportunity! Profit: {}", opportunity.estimated_profit);
                
                // Log the opportunity to file
                self.log_opportunity(&opportunity);
                
                // Send opportunity for execution
                if let Err(e) = self.opportunity_tx.send(opportunity).await {
                    eprintln!("❌ Failed to send arbitrage opportunity: {}", e);
                }
            }
        }
        Ok(())
    }

    /// Decode a transaction to extract swap information from pool addresses
    async fn decode_pool_swap_transaction(&self, tx: &Transaction) -> Option<DecodedSwap> {
        // Check if transaction is to any monitored pool
        if let Some(to) = tx.to {
            if !self.monitored_pools.contains(&to) {
                return None;
            }

            // This is a transaction to a monitored pool
            // Extract swap information from transaction input or logs
            if let Some(swap_info) = self.decode_pool_swap_input(&tx.input, &to) {
                if let Some(block_number) = tx.block_number {
                    if let Ok(Some(block)) = self.provider.get_block(block_number).await {
                        let timestamp = block.timestamp.as_u64();
                        return Some(DecodedSwap {
                            tx_hash: H160::from_slice(&tx.hash.as_bytes()[0..20]),
                            pool_address: to,
                            token_x: swap_info.token_x,
                            token_x_amount: swap_info.token_x_amount,
                            block_number: block_number.as_u64(),
                            timestamp,
                        });
                    }
                }
            }
        }

        None
    }

    /// Decode pool swap input data to extract token and amount information
    fn decode_pool_swap_input(&self, input: &[u8], pool_address: &H160) -> Option<SwapInfo> {
        // Get pool info from reserve_cache
        let pool_entry = self.reserve_cache.get(pool_address)?;
        let pool_data = pool_entry.value();
        
        // Extract token0 and token1 from pool data
        let token0 = pool_data.token0;
        let token1 = pool_data.token1;

        // For now, we'll use a simplified approach:
        // Assume the transaction is buying token1 (tokenX) with token0
        // In a real implementation, you'd decode the actual swap parameters
        
        if input.len() < 4 {
            return None;
        }

        // Check for common swap function selectors
        let method_id = &input[0..4];
        
        // swap function selector: 0xa9059cbb (transfer)
        // or other pool-specific swap functions
        if method_id == [0xa9, 0x05, 0x9c, 0xbb] || 
           method_id == [0x23, 0xb8, 0x72, 0xdd] || // transferFrom
           method_id == [0x38, 0xed, 0x17, 0x39] {  // swapExactTokensForTokens
            
            // Extract amount from input (simplified)
            if input.len() >= 4 + 32 {
                let amount_bytes = &input[4..4 + 32];
                let amount = U256::from_big_endian(amount_bytes);
                
                // For arbitrage, we're interested in the token being bought
                // Assume it's token1 (you might need to determine this from the actual swap direction)
                return Some(SwapInfo {
                    pool_address: *pool_address,
                    token_x: token1,
                    token_x_amount: amount,
                });
            }
        }

        None
    }

    /// Decode Sync event to extract swap information
    async fn decode_sync_event(&self, log: &Log) -> Option<DecodedSwap> {
        // Sync event: Sync(uint112 reserve0, uint112 reserve1)
        if log.topics.len() != 1 || log.data.len() != 64 {
            return None;
        }

        let pool_address = log.address;
        
        // Get pool info from reserve_cache
        let pool_entry = self.reserve_cache.get(&pool_address)?;
        let pool_data = pool_entry.value();
        
        // Extract new reserves from event data
        let reserve0_bytes = &log.data[0..32];
        let reserve1_bytes = &log.data[32..64];
        
        let new_reserve0 = U256::from_big_endian(reserve0_bytes);
        let new_reserve1 = U256::from_big_endian(reserve1_bytes);
        
        // Get old reserves from cache
        let old_reserve0 = pool_data.reserve0.unwrap_or(U256::zero());
        let old_reserve1 = pool_data.reserve1.unwrap_or(U256::zero());
        
        // Calculate which token was bought/sold based on reserve changes
        let token0_change = new_reserve0.saturating_sub(old_reserve0);
        let token1_change = new_reserve1.saturating_sub(old_reserve1);
        
        // Determine swap direction and amount
        let (token_x, token_x_amount) = if token0_change > U256::zero() {
            // token0 was bought (reserve0 increased)
            (pool_data.token0, token0_change)
        } else if token1_change > U256::zero() {
            // token1 was bought (reserve1 increased)
            (pool_data.token1, token1_change)
        } else {
            return None; // No clear swap direction
        };

        // Get current block info
        if let Ok(Some(block)) = self.provider.get_block(log.block_number.unwrap_or(U64::zero())).await {
            let timestamp = block.timestamp.as_u64();
            return Some(DecodedSwap {
                tx_hash: H160::zero(), // Sync events don't have direct tx hash
                pool_address,
                token_x,
                token_x_amount,
                block_number: log.block_number.unwrap_or(U64::zero()).as_u64(),
                timestamp,
            });
        }

        None
    }

    /// Find arbitrage opportunities for a decoded swap
    async fn find_arbitrage_opportunity(&self, decoded_swap: &DecodedSwap) -> Option<ArbitrageOpportunity> {
        // Get token index
        let token_x_index = self.token_index.address_to_index.get(&decoded_swap.token_x)?;
        let token_x_index_u32 = *token_x_index as u32;
        
        println!("🔍 Finding arbitrage for tokenX (idx {}): {}", token_x_index, decoded_swap.token_x);

        // Get all routes that contain this token and the affected pool
        let candidate_routes = self.precomputed_route_cache
            .get(&token_x_index_u32)
            .map(|entry| entry.value().clone())
            .unwrap_or_default();

        println!("📊 Found {} candidate routes for tokenX", candidate_routes.len());

        // Filter routes that contain the affected pool
        let filtered_routes: Vec<&RoutePath> = candidate_routes.iter()
            .filter(|route| route.pools.contains(&decoded_swap.pool_address))
            .collect();

        println!("🎯 {} routes contain the affected pool {}", filtered_routes.len(), decoded_swap.pool_address);

        if filtered_routes.is_empty() {
            return None;
        }

        // Simulate all filtered routes in parallel
        let simulation_results: Vec<Option<SimulatedRoute>> = filtered_routes.par_iter()
            .map(|route| {
                // Split route into buy/sell paths
                let (buy_path, sell_path) = split_route_around_token_x(route, token_x_index_u32)?;
                
                // Simulate buy path (base -> tokenX)
                let buy_amounts = simulate_buy_path_amounts_array(
                    &buy_path, 
                    decoded_swap.token_x_amount, 
                    &self.reserve_cache, 
                    &self.token_index
                )?;

                // Simulate sell path (tokenX -> base)
                let sell_amounts = simulate_sell_path_amounts_array(
                    &sell_path, 
                    decoded_swap.token_x_amount, 
                    &self.reserve_cache, 
                    &self.token_index
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
                            .map(|&idx| self.token_index_to_symbol(idx))
                            .collect();

                        // Merge pools
                        let mut merged_pools = buy_path.pools.clone();
                        merged_pools.extend_from_slice(&sell_path.pools);

                        return Some(SimulatedRoute {
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
        let profitable_routes: Vec<SimulatedRoute> = simulation_results.into_iter()
            .filter_map(|r| r)
            .collect();

        println!("💰 Found {} profitable routes", profitable_routes.len());

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

    /// Helper to map token index to symbol
    fn token_index_to_symbol(&self, idx: u32) -> String {
        if let Some(addr) = self.token_index.index_to_address.get(&(idx as u32)) {
            format!("0x{:x}", addr)
        } else {
            format!("token{}", idx)
        }
    }

    /// Log profitable arbitrage opportunity to file
    fn log_opportunity(&self, opportunity: &ArbitrageOpportunity) {
        let now: DateTime<Utc> = Utc::now();
        
        // Create detailed log entry
        let log_entry = json!({
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
            .open(&self.log_file_path) {
            
            if let Err(e) = writeln!(file, "{}", serde_json::to_string_pretty(&log_entry).unwrap()) {
                eprintln!("❌ Failed to write to log file: {}", e);
            }
        } else {
            eprintln!("❌ Failed to open log file: {}", self.log_file_path);
        }

        // Also print summary to console
        println!("📝 Logged opportunity to: {}", self.log_file_path);
    }

    /// Get hourly profit summary from log file
    pub fn get_hourly_profit_summary(&self) -> Result<String, Box<dyn std::error::Error>> {
        let mut total_profit = U256::zero();
        let mut opportunity_count = 0;
        let mut hourly_profits: HashMap<u32, U256> = HashMap::new(); // hour -> total profit

        if let Ok(content) = std::fs::read_to_string(&self.log_file_path) {
            for line in content.lines() {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(profit_str) = entry["estimated_profit"].as_str() {
                        if let Ok(profit) = U256::from_dec_str(profit_str) {
                            total_profit = total_profit.saturating_add(profit);
                            opportunity_count += 1;

                            // Group by hour
                            if let Some(timestamp) = entry["timestamp"].as_str() {
                                if let Ok(dt) = DateTime::parse_from_rfc3339(timestamp) {
                                    let hour = dt.hour();
                                    *hourly_profits.entry(hour).or_insert(U256::zero()) = 
                                        hourly_profits.get(&hour).unwrap_or(&U256::zero()).saturating_add(profit);
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut summary = format!(
            "📊 ARBITRAGE SUMMARY:\nTotal Opportunities: {}\nTotal Estimated Profit: {}\n\nHourly Breakdown:\n",
            opportunity_count, total_profit
        );

        for hour in 0..24 {
            if let Some(&profit) = hourly_profits.get(&hour) {
                summary.push_str(&format!("Hour {}: {} profit\n", hour, profit));
            }
        }

        Ok(summary)
    }
}

/// Swap information extracted from transaction
#[derive(Debug, Clone)]
struct SwapInfo {
    pool_address: H160,
    token_x: H160,
    token_x_amount: U256,
}

/// Start mempool monitoring service
pub async fn start_mempool_monitoring(
    provider: Arc<Provider<Ws>>,
    reserve_cache: Arc<ReserveCache>,
    token_index: Arc<TokenIndexMap>,
    precomputed_route_cache: Arc<DashMap<u32, Vec<RoutePath>>>,
    config: Config,
) -> Result<mpsc::Receiver<ArbitrageOpportunity>, Box<dyn std::error::Error>> {
    let (opportunity_tx, opportunity_rx) = mpsc::channel(100);
    
    let decoder = MempoolDecoder::new(
        provider,
        reserve_cache,
        token_index,
        precomputed_route_cache,
        config,
        opportunity_tx,
    );

    // Start monitoring in background
    tokio::spawn(async move {
        if let Err(e) = decoder.start_monitoring().await {
            eprintln!("❌ Mempool monitoring failed: {}", e);
        }
    });

    Ok(opportunity_rx)
}
