use std::fs;
use ethers::types::{H160, U256};
use ethers::providers::{Provider, Http};
use std::sync::Arc;
use serde::{Deserialize, Serialize};

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

// Known token addresses with USD values (for liquidity calculation)
const KNOWN_TOKENS: &[(&str, &str, f64)] = &[
    // BNB
    ("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", "BNB", 600.0),
    // USDT
    ("0x55d398326f99059fF775485246999027B3197955", "USDT", 1.0),
    // USDC
    ("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d", "USDC", 1.0),
    // BUSD
    ("0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56", "BUSD", 1.0),
    // CAKE
    ("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82", "CAKE", 2.0),
];

fn get_token_usd_value(token_address: &H160) -> Option<f64> {
    let addr_str = format!("0x{:x}", token_address);
    println!("   Looking for address: {}", addr_str);
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
    
    let total_liquidity = liquidity0_usd + liquidity1_usd;
    
    println!("🔍 Liquidity Debug:");
    println!("   Token0: {} (price: ${})", format!("0x{:x}", token0), price0);
    println!("   Token1: {} (price: ${})", format!("0x{:x}", token1), price1);
    println!("   Reserve0: {} (decimals: {})", reserve0, token0_decimals);
    println!("   Reserve1: {} (decimals: {})", reserve1, token1_decimals);
    println!("   Reserve0_f64: {}", reserve0_f64);
    println!("   Reserve1_f64: {}", reserve1_f64);
    println!("   Liquidity0_USD: ${}", liquidity0_usd);
    println!("   Liquidity1_USD: ${}", liquidity1_usd);
    println!("   Total Liquidity: ${}", total_liquidity);
    
    total_liquidity
}

async fn check_v2_liquidity_debug(
    pair: &PairInfo,
    provider: &Arc<Provider<Http>>,
) -> Option<f64> {
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
            
            Some(liquidity_usd)
        }
        Err(e) => {
            eprintln!("Error checking V2 liquidity for {}: {:?}", pair.pair_address, e);
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Debugging liquidity calculation...");
    
    // Initialize provider
    let rpc_url = std::env::var("BSC_RPC_URL").unwrap_or_else(|_| {
        "http://localhost:8545/".to_string()
    });
    let provider = Arc::new(Provider::<Http>::try_from(&rpc_url)?);
    
    println!("🔗 Using RPC: {}", rpc_url);
    
    // Test with first few pairs from the file
    let v2_file = std::fs::File::open("data/pairs_v2.jsonl")?;
    let v2_reader = std::io::BufReader::new(v2_file);
    
    let mut test_count = 0;
    for line in std::io::BufRead::lines(v2_reader) {
        if test_count >= 5 { break; } // Test only first 5 pairs
        
        let line = line?;
        if line.trim().is_empty() { continue; }
        
        let pair: PairInfo = serde_json::from_str(&line)?;
        
        println!("\n📊 Testing pair: {}", pair.pair_address);
        println!("   DEX: {}", pair.dex_name);
        println!("   Token0: {} ({})", 
                 pair.token0_symbol.as_deref().unwrap_or("Unknown"), 
                 format!("0x{:x}", pair.token0));
        println!("   Token1: {} ({})", 
                 pair.token1_symbol.as_deref().unwrap_or("Unknown"), 
                 format!("0x{:x}", pair.token1));
        
        if let Some(liquidity_usd) = check_v2_liquidity_debug(&pair, &provider).await {
            println!("✅ Liquidity: ${:.2}", liquidity_usd);
            if liquidity_usd >= 1000.0 {
                println!("✅ PASSES threshold");
            } else {
                println!("❌ FAILS threshold");
            }
        } else {
            println!("❌ Could not get liquidity");
        }
        
        test_count += 1;
        
        // Small delay between requests
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    
    Ok(())
} 