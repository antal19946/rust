use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::collections::HashMap;
use ethers::types::{H160, U256};
use ethers::providers::{Provider, Http};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use futures::stream::{self, StreamExt};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::fs;
use rayon::prelude::*;
use futures::stream::FuturesUnordered;
use tokio::sync::Semaphore;

#[derive(Debug, Deserialize, Serialize, Clone)]
struct PairInfo {
    pair_address: H160,
    token0: H160,
    token1: H160,
    dex_name: String,
    dex_version: String,
    token0_symbol: Option<String>,
    token1_symbol: Option<String>,
    token0_decimals: Option<u8>,
    token1_decimals: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct LiquidPairInfo {
    pair_address: H160,
    token0: H160,
    token1: H160,
    dex_name: String,
    dex_version: String,
    token0_symbol: Option<String>,
    token1_symbol: Option<String>,
    token0_decimals: Option<u8>,
    token1_decimals: Option<u8>,
    liquidity_usd: f64,
    reserve0: U256,
    reserve1: U256,
}

// Known token addresses with USD values (for liquidity calculation) - Updated 2024 prices
const KNOWN_TOKENS: &[(&str, &str, f64)] = &[
    ("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", "BNB", 693.73),
    ("0x2170Ed0880ac9A755fd29B2688956BD959F933F8", "ETH", 2976.82),
    ("0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c", "BTC", 118079.0),
    ("0x1D2F0da169ceB9fC7B3144628dB156f3F6c60dBE", "XRP", 2.84),
    ("0x3EE2200Efb3400fAbB9AacF31297cBdD1d435D47", "ADA", 0.73144),
    ("0x4338665CBB7B2485A8855A139b75D5e34AB0DB94", "LTC", 93.81),
    ("0x8fF795a6F4D97E7887C79beA79aba5cc76444aDf", "BCH", 522.68),
    ("0x7083609fCE4d1d8Dc0C979AAb8c869Ea2C873402", "DOT", 4.00),
    ("0xF8A0BF9cF54Bb92F17374d9e9A321E6a111a51bD", "LINK", 15.36),
    ("0x1CE0c2827e2eF14D5C4f29a091d735A204794041", "AVAX", 20.97),
    ("0x0D8Ce2A99Bb6e3B7Db580eD848240e4a0F9aE153", "FIL", 2.58),
    ("0x16939ef78684453bfDFb47825F0a1C2EeA8c8c8b", "MATIC", 0.234835),
    ("0x56b6fB708fC5732DEC1Afc8D8556423A2EDcCbD6", "EOS", 0.539445),
    ("0x67ee3Cb086F8a16f34beE3ca72FAD36F7Db929e2", "DYDX", 0.608177),
    ("0x9f589e3eabe42ebC94A44757767D194A1EdEfc2C", "TUSD", 1.00),
    ("0x55d398326f99059fF775485246999027B3197955", "USDT", 1.00),
    ("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d", "USDC", 1.00),
    ("0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56", "BUSD", 1.00),
    ("0x14016E85a25aeb13065688cAFB43044C2ef86784", "TUSD", 1.00),
    ("0x3F56e0c36d275367b8C502090EDF38289a3dEa0d", "MAI", 1.00),
    ("0x4BD17003473389A42DAF6a0a729f6Fdb328BbBd7", "VAI", 1.00),
    ("0x1AF3F329e8BE154074D8769D1FFa4eE058B1DBc3", "DAI", 0.9999),
    ("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82", "CAKE", 2.51),
    ("0x47BEAd2563dCBf3bF2c9407fEa4dC236fAbA485A", "SXP", 0.199256),
    ("0x4e0e3383324AA6A2c8F2E5412B8E9a195309e790", "ALICE", 0.45562),
    ("0x715D400Fc88a5a4b7b4C8C8C8C8C8C8C8C8C8C8", "AXS", 2.51),
];

fn get_token_usd_value(token_address: &H160) -> Option<f64> {
    let addr_str = format!("0x{:x}", token_address);
    KNOWN_TOKENS.iter()
        .find(|(addr, _, _)| addr.to_lowercase() == addr_str.to_lowercase())
        .map(|(_, _, price)| *price)
}

fn calculate_liquidity_usd(
    reserve0: U256,
    reserve1: U256,
    token0: &H160,
    token1: &H160,
    token0_decimals: u8,
    token1_decimals: u8,
) -> f64 {
    let price0 = get_token_usd_value(token0).unwrap_or(0.0);
    let price1 = get_token_usd_value(token1).unwrap_or(0.0);
    
    let reserve0_f64 = reserve0.as_u128() as f64 / 10_f64.powi(token0_decimals as i32);
    let reserve1_f64 = reserve1.as_u128() as f64 / 10_f64.powi(token1_decimals as i32);
    
    let liquidity0_usd = reserve0_f64 * price0;
    let liquidity1_usd = reserve1_f64 * price1;
    
    liquidity0_usd + liquidity1_usd
}

async fn check_v2_liquidity(
    pair: &PairInfo,
    provider: &Arc<Provider<Http>>,
) -> Option<LiquidPairInfo> {
    // Load ABI from file
    let abi_str = fs::read_to_string("abi/uniswap_v2_pair.json").expect("Could not read V2 ABI");
    let abi: ethers::abi::Abi = serde_json::from_str(&abi_str).expect("Invalid ABI JSON");
    let contract = ethers::contract::Contract::new(
        pair.pair_address,
        abi,
        provider.clone(),
    );
    // Call getReserves function
    let result: Result<(U256, U256, u32), _> = contract
        .method("getReserves", ())
        .unwrap()
        .call()
        .await;
    match result {
        Ok((reserve0, reserve1, _)) => {
            let token0_decimals = pair.token0_decimals.unwrap_or(18);
            let token1_decimals = pair.token1_decimals.unwrap_or(18);
            let liquidity_usd = calculate_liquidity_usd(
                reserve0,
                reserve1,
                &pair.token0,
                &pair.token1,
                token0_decimals,
                token1_decimals,
            );
            if liquidity_usd >= 9.0 {
                Some(LiquidPairInfo {
                    pair_address: pair.pair_address,
                    token0: pair.token0,
                    token1: pair.token1,
                    dex_name: pair.dex_name.clone(),
                    dex_version: pair.dex_version.clone(),
                    token0_symbol: pair.token0_symbol.clone(),
                    token1_symbol: pair.token1_symbol.clone(),
                    token0_decimals: pair.token0_decimals,
                    token1_decimals: pair.token1_decimals,
                    liquidity_usd,
                    reserve0,
                    reserve1,
                })
            } else {
                None
            }
        }
        Err(e) => {
            eprintln!("Error checking V2 liquidity for {}: {:?}", pair.pair_address, e);
            None
        }
    }
}

// Process a batch of pairs in parallel
async fn process_batch(
    batch: Vec<PairInfo>,
    provider: Arc<Provider<Http>>,
    semaphore: Arc<Semaphore>,
) -> Vec<LiquidPairInfo> {
    let mut futures = FuturesUnordered::new();
    
    for pair in batch {
        let provider = provider.clone();
        let semaphore = semaphore.clone();
        
        let future = async move {
            let _permit = semaphore.acquire().await.unwrap();
            check_v2_liquidity(&pair, &provider).await
        };
        
        futures.push(future);
    }
    
    let mut results = Vec::new();
    while let Some(result) = futures.next().await {
        if let Some(liquid_pair) = result {
            results.push(liquid_pair);
        }
    }
    
    results
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Starting ACCURATE liquidity-based pair filtering (V2 only) - OPTIMIZED for 19-core CPU...");
    
    // Initialize provider
    let rpc_url = std::env::var("BSC_RPC_URL").unwrap_or_else(|_| {
        "http://localhost:8545/".to_string()  // Local BSC geth node
    });
    let provider = Arc::new(Provider::<Http>::try_from(&rpc_url)?);
    
    let min_liquidity_usd = 9.0;
    println!("📊 Minimum liquidity threshold: ${}", min_liquidity_usd);
    println!("🔗 Using RPC: {}", rpc_url);
    
    // Load all pairs first
    println!("📖 Loading all V2 pairs...");
    let mut all_pairs = Vec::new();
    let v2_file = File::open("data/pairs_v2.jsonl")?;
    let v2_reader = BufReader::new(v2_file);
    
    for line in v2_reader.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        
        let pair: PairInfo = serde_json::from_str(&line)?;
        all_pairs.push(pair);
    }
    
    let total_pairs = all_pairs.len();
    println!("📊 Loaded {} V2 pairs", total_pairs);
    
    // Configure parallel processing - higher limits for local node
    let batch_size = 2000; // Process 2000 pairs at a time
    let max_concurrent = 200; // Max 200 concurrent RPC calls (local node can handle more)
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    
    println!("⚡ Processing in batches of {} with {} concurrent requests", batch_size, max_concurrent);
    
    let start_time = std::time::Instant::now();
    let mut liquid_pairs = Vec::new();
    let mut processed_count = 0;
    
    // Process pairs in batches
    for (batch_idx, batch) in all_pairs.chunks(batch_size).enumerate() {
        let batch_vec = batch.to_vec();
        let batch_results = process_batch(batch_vec, provider.clone(), semaphore.clone()).await;
        
        let batch_liquid_count = batch_results.len();
        liquid_pairs.extend(batch_results);
        processed_count += batch.len();
        
        let progress = (processed_count as f64 / total_pairs as f64 * 100.0) as i32;
        let elapsed = start_time.elapsed();
        let rate = processed_count as f64 / elapsed.as_secs_f64();
        
        println!("✅ Batch {}: Found {} liquid pairs, Progress: {}% ({}/{}), Rate: {:.1} pairs/sec", 
                 batch_idx + 1, batch_liquid_count, progress, processed_count, total_pairs, rate);
    }
    
    // Write results
    println!("📝 Writing results...");
    let output_filename = "data/liquid_pairs_v2_new.jsonl";
    let mut output = BufWriter::new(File::create(output_filename)?);
    for liquid_pair in &liquid_pairs {
        let json = serde_json::to_string(liquid_pair)?;
        writeln!(output, "{}", json)?;
    }
    
    let total_time = start_time.elapsed();
    println!("🎉 ACCURATE filtering completed!");
    println!("📊 Results:");
    println!("   V2 pairs: {} total → {} liquid ({}%)", 
             total_pairs, liquid_pairs.len(), 
             (liquid_pairs.len() as f64 / total_pairs as f64 * 100.0) as i32);
    println!("   Total time: {:.2?}", total_time);
    println!("   Average speed: {:.1} pairs/sec", total_pairs as f64 / total_time.as_secs_f64());
    println!("📁 Output file:");
    println!("   - {}", output_filename);
    
    Ok(())
} 