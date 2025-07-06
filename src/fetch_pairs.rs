use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use ethers::{
    providers::{Http, Provider, Middleware},
    types::{Address, BlockNumber, Filter, Log, H256},
    utils::hex,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use std::str::FromStr;
use ethers::utils::keccak256;

use crate::config::{Config, DexConfig, DexVersion};

/// Pair information structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairInfo {
    pub pair_address: Address,
    pub token0: Address,
    pub token1: Address,
    pub dex_name: String,
    pub dex_version: DexVersion,
    pub factory_address: Address,
    pub block_number: u64,
    pub transaction_hash: String,
}

/// Progress tracking for each factory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryProgress {
    pub factory_address: Address,
    pub dex_name: String,
    pub last_scanned_block: u64,
    pub total_pairs: u64,
    pub last_updated: u64, // timestamp
}

/// Main pair fetcher
pub struct PairFetcher {
    config: Config,
    progress_file: String,
    v2_pairs_file: String,
    v3_pools_file: String,
    progress: Arc<Mutex<HashMap<Address, FactoryProgress>>>,
    safe_tokens: Arc<HashSet<Address>>,
}

impl PairFetcher {
    pub fn new(config: Config) -> Self {
        let progress_file = "data/factory_progress.json".to_string();
        let v2_pairs_file = "data/pairs_v2.jsonl".to_string();
        let v3_pools_file = "data/pairs_v3.jsonl".to_string();
        
        // Create data directory if it doesn't exist
        std::fs::create_dir_all("data").ok();
        
        // Load safe tokens
        let safe_tokens = load_safe_tokens("data/safe_tokens.json");
        
        Self {
            config,
            progress_file,
            v2_pairs_file,
            v3_pools_file,
            progress: Arc::new(Mutex::new(HashMap::new())),
            safe_tokens: Arc::new(safe_tokens),
        }
    }
    
    /// Load existing progress from file
    pub fn load_progress(&self) -> Result<()> {
        if Path::new(&self.progress_file).exists() {
            let file = File::open(&self.progress_file)?;
            let metadata = file.metadata()?;
            if metadata.len() == 0 {
                // Empty file, treat as no progress
                *self.progress.lock().unwrap() = HashMap::new();
                println!("Progress file is empty, starting fresh.");
            } else {
                let progress: HashMap<Address, FactoryProgress> = serde_json::from_reader(file)?;
                let len = progress.len();
                *self.progress.lock().unwrap() = progress;
                println!("Loaded progress for {} factories", len);
            }
        }
        Ok(())
    }
    
    /// Save progress to file
    pub fn save_progress(&self) -> Result<()> {
        let progress = self.progress.lock().unwrap();
        let file = File::create(&self.progress_file)?;
        serde_json::to_writer_pretty(file, &*progress)?;
        Ok(())
    }
    
    /// Get or create progress for a factory
    fn get_or_create_progress(&self, factory_address: Address, dex_name: &str) -> FactoryProgress {
        let mut progress = self.progress.lock().unwrap();
        
        if let Some(existing) = progress.get(&factory_address) {
            existing.clone()
        } else {
            let new_progress = FactoryProgress {
                factory_address,
                dex_name: dex_name.to_string(),
                last_scanned_block: 0,
                total_pairs: 0,
                last_updated: chrono::Utc::now().timestamp() as u64,
            };
            progress.insert(factory_address, new_progress.clone());
            new_progress
        }
    }
    
    /// Update progress for a factory
    fn update_progress(&self, factory_address: Address, last_block: u64, new_pairs: u64) {
        let mut progress = self.progress.lock().unwrap();
        if let Some(factory_progress) = progress.get_mut(&factory_address) {
            factory_progress.last_scanned_block = last_block;
            factory_progress.total_pairs += new_pairs;
            factory_progress.last_updated = chrono::Utc::now().timestamp() as u64;
        }
    }
    
    /// Save pair to appropriate file (V2 or V3)
    fn save_pair(&self, pair: &PairInfo) -> Result<()> {
        // Only save if token0 or token1 is in safe_tokens
        if !self.safe_tokens.contains(&pair.token0) || !self.safe_tokens.contains(&pair.token1) {
            return Ok(()); // skip
        }
        let file_path = match pair.dex_version {
            DexVersion::V2 => &self.v2_pairs_file,
            DexVersion::V3 => &self.v3_pools_file,
        };
        
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_path)?;
        
        let mut writer = BufWriter::new(file);
        let json_line = serde_json::to_string(pair)?;
        writeln!(writer, "{}", json_line)?;
        writer.flush()?;
        
        Ok(())
    }
    
    /// Fetch pairs from a single factory
    async fn fetch_factory_pairs(
        &self,
        dex: &DexConfig,
        provider: &Provider<Http>,
    ) -> Result<Vec<PairInfo>> {
        let mut pairs = Vec::new();
        let progress = self.get_or_create_progress(dex.factory_address, &dex.name);
        println!("Fetching pairs from {} (last block: {})", dex.name, progress.last_scanned_block);
        // Get current block number
        let current_block = provider.get_block_number().await?.as_u64();
        let from_block = if progress.last_scanned_block == 0 {
            // First time scanning - start from a reasonable block
            match dex.version {
                DexVersion::V2 => 1_000_000, // BSC started around this block
                DexVersion::V3 => 27_000_000, // Pancake V3 started around this block
            }
        } else {
            progress.last_scanned_block + 1
        };
        if from_block >= current_block {
            println!("{} is up to date", dex.name);
            return Ok(pairs);
        }
        // Create filter for PairCreated events
        let filter = match dex.version {
            DexVersion::V2 => Filter::new()
                .from_block(BlockNumber::Number(from_block.into()))
                .to_block(BlockNumber::Latest)
                .address(dex.factory_address)
                .topic0(H256::from_str("0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9").unwrap()),
            DexVersion::V3 => {
                let event_sig = "PoolCreated(address,address,uint24,int24,address)";
                let event_topic = H256::from_slice(keccak256(event_sig.as_bytes()).as_slice());
                Filter::new()
                    .from_block(BlockNumber::Number(from_block.into()))
                    .to_block(BlockNumber::Latest)
                    .address(dex.factory_address)
                    .topic0(event_topic)
            },
        };
        // Fetch logs in batches to avoid timeout
        let batch_size = 50000;
        let mut current_from = from_block;
        while current_from < current_block {
            let current_to = std::cmp::min(current_from + batch_size - 1, current_block);
            let batch_filter = filter.clone()
                .from_block(BlockNumber::Number(current_from.into()))
                .to_block(BlockNumber::Number(current_to.into()));
            match provider.get_logs(&batch_filter).await {
                Ok(logs) => {
                    if dex.version == DexVersion::V3 {
                        println!("[DEBUG] V3 PoolCreated logs fetched: {} (blocks {}-{})", logs.len(), current_from, current_to);
                        for (i, log) in logs.iter().enumerate().take(3) {
                            let token0 = Address::from_slice(&log.topics[1].as_bytes()[12..]);
                            let token1 = Address::from_slice(&log.topics[2].as_bytes()[12..]);
                            println!("[DEBUG] V3 log {}: token0 = {:?}, token1 = {:?}", i, token0, token1);
                        }
                    }
                    let mut before_filter = 0;
                    let mut after_filter = 0;
                    for log in logs {
                        let pair = match dex.version {
                            DexVersion::V2 => self.parse_pair_created_log(&log, dex).await?,
                            DexVersion::V3 => self.parse_pool_created_log(&log, dex).await?,
                        };
                        if let Some(pair) = pair {
                            before_filter += 1;
                            // Only save if token0 or token1 is in safe_tokens
                            if self.safe_tokens.contains(&pair.token0) || self.safe_tokens.contains(&pair.token1) {
                                after_filter += 1;
                                pairs.push(pair.clone());
                                self.save_pair(&pair)?;
                            }
                        }
                    }
                    if dex.version == DexVersion::V3 {
                        println!("[DEBUG] V3 pairs before safe token filter: {}", before_filter);
                        println!("[DEBUG] V3 pairs after safe token filter: {}", after_filter);
                    }
                    // Update progress after each batch
                    self.update_progress(dex.factory_address, current_to, pairs.len() as u64);
                    self.save_progress()?;
                    println!("{}: Scanned blocks {}-{}, found {} pairs", 
                        dex.name, current_from, current_to, pairs.len());
                }
                Err(e) => {
                    eprintln!("Error fetching logs for {}: {}", dex.name, e);
                    // Continue with next batch
                }
            }
            current_from = current_to + 1;
            // Small delay to avoid rate limiting
            sleep(Duration::from_millis(100)).await;
        }
        Ok(pairs)
    }
    
    /// Parse PairCreated log for V2 DEXes
    async fn parse_pair_created_log(&self, log: &Log, dex: &DexConfig) -> Result<Option<PairInfo>> {
        if dex.version != DexVersion::V2 {
            return Ok(None);
        }
        
        // PairCreated event signature: PairCreated(address indexed token0, address indexed token1, address pair, uint allPairsLength)
        if log.topics.len() < 3 {
            return Ok(None);
        }
        
        let token0 = Address::from_slice(&log.topics[1].as_bytes()[12..]);
        let token1 = Address::from_slice(&log.topics[2].as_bytes()[12..]);
        
        // Extract pair address from data
        if log.data.len() < 32 {
            return Ok(None);
        }
        
        let pair_address = Address::from_slice(&log.data[12..32]);
        
        let pair_info = PairInfo {
            pair_address,
            token0,
            token1,
            dex_name: dex.name.clone(),
            dex_version: dex.version.clone(),
            factory_address: dex.factory_address,
            block_number: log.block_number.unwrap().as_u64(),
            transaction_hash: format!("0x{}", hex::encode(log.transaction_hash.unwrap())),
        };
        
        Ok(Some(pair_info))
    }
    
    /// Parse PoolCreated log for V3 DEXes
    async fn parse_pool_created_log(&self, log: &Log, dex: &DexConfig) -> Result<Option<PairInfo>> {
        if dex.version != DexVersion::V3 {
            return Ok(None);
        }
        
        // PoolCreated event signature: PoolCreated(address indexed token0, address indexed token1, uint24 indexed fee, address pool, ...)
        if log.topics.len() < 3 {
            return Ok(None);
        }
        
        let token0 = Address::from_slice(&log.topics[1].as_bytes()[12..]);
        let token1 = Address::from_slice(&log.topics[2].as_bytes()[12..]);
        
        // Extract pool address from data (correct offset: [44..64])
        if log.data.len() < 64 {
            return Ok(None);
        }
        let pool_address = Address::from_slice(&log.data[44..64]);
        
        let pair_info = PairInfo {
            pair_address: pool_address,
            token0,
            token1,
            dex_name: dex.name.clone(),
            dex_version: dex.version.clone(),
            factory_address: dex.factory_address,
            block_number: log.block_number.unwrap().as_u64(),
            transaction_hash: format!("0x{}", hex::encode(log.transaction_hash.unwrap())),
        };
        
        Ok(Some(pair_info))
    }
    
    /// Main function to fetch all pairs from all factories
    pub async fn fetch_all_pairs(&self) -> Result<()> {
        println!("Starting pair fetching for {} DEXes...", self.config.dexes.len());
        
        // Load existing progress
        self.load_progress()?;
        
        // Create HTTP provider
        let provider = Provider::<Http>::try_from(&self.config.rpc_url)?;
        
        // Process all DEXes in parallel
        let results: Vec<Result<Vec<PairInfo>>> = self.config.dexes
            .par_iter()
            .map(|dex| {
                let provider = provider.clone();
                let fetcher = self.clone();
                
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        fetcher.fetch_factory_pairs(dex, &provider).await
                    })
            })
            .collect();
        
        // Process results and save progress
        let mut total_pairs = 0;
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(pairs) => {
                    println!("{}: Found {} pairs", self.config.dexes[i].name, pairs.len());
                    total_pairs += pairs.len();
                }
                Err(e) => {
                    eprintln!("Error fetching pairs from {}: {}", self.config.dexes[i].name, e);
                }
            }
        }
        
        // Save final progress
        self.save_progress()?;
        
        println!("Pair fetching completed! Total pairs found: {}", total_pairs);
        println!("V2 pairs saved to: {}", self.v2_pairs_file);
        println!("V3 pools saved to: {}", self.v3_pools_file);
        println!("Progress saved to: {}", self.progress_file);
        
        Ok(())
    }
}

impl Clone for PairFetcher {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            progress_file: self.progress_file.clone(),
            v2_pairs_file: self.v2_pairs_file.clone(),
            v3_pools_file: self.v3_pools_file.clone(),
            progress: self.progress.clone(),
            safe_tokens: self.safe_tokens.clone(),
        }
    }
}

fn load_safe_tokens(path: &str) -> HashSet<Address> {
    let mut set = HashSet::new();
    if let Ok(file) = File::open(path) {
        if let Ok(tokens) = serde_json::from_reader::<_, serde_json::Value>(file) {
            if let Some(arr) = tokens.as_array() {
                for token in arr {
                    if let Some(addr) = token.get("address").and_then(|a| a.as_str()) {
                        if let Ok(address) = addr.parse::<Address>() {
                            set.insert(address);
                        }
                    }
                }
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_pair_info_serialization() {
        let pair = PairInfo {
            pair_address: Address::random(),
            token0: Address::random(),
            token1: Address::random(),
            dex_name: "TestDEX".to_string(),
            dex_version: DexVersion::V2,
            factory_address: Address::random(),
            block_number: 12345,
            transaction_hash: "0x1234567890abcdef".to_string(),
        };
        
        let json = serde_json::to_string(&pair).unwrap();
        let deserialized: PairInfo = serde_json::from_str(&json).unwrap();
        
        assert_eq!(pair.dex_name, deserialized.dex_name);
        assert_eq!(pair.block_number, deserialized.block_number);
    }
}
