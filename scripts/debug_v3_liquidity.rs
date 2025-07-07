use std::fs;
use ethers::types::{H160, U256};
use ethers::providers::{Provider, Http};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use ethers::contract::abigen;

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

// Known token addresses with USD values (for liquidity calculation) - Updated 2024 prices
const KNOWN_TOKENS: &[(&str, &str, f64)] = &[
    // BNB - Current price ~$600
    ("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", "BNB", 600.0),
    // USDT - Stable at $1
    ("0x55d398326f99059fF775485246999027B3197955", "USDT", 1.0),
    // USDC - Stable at $1
    ("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d", "USDC", 1.0),
    // BUSD - Stable at $1
    ("0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56", "BUSD", 1.0),
    // CAKE - Current price ~$2.5
    ("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82", "CAKE", 2.5),
    // ETH - Current price ~$3500
    ("0x2170Ed0880ac9A755fd29B2688956BD959F933F8", "ETH", 3500.0),
    // BTC - Current price ~$65000
    ("0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c", "BTC", 65000.0),
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

// V3 liquidity calculation from sqrt_price_x96 and liquidity
fn calculate_v3_reserves(
    sqrt_price_x96: U256,
    liquidity: U256,
    token0_decimals: u8,
    token1_decimals: u8,
) -> (U256, U256) {
    // Conservative estimate:
    // token0 ≈ liquidity / sqrtPriceX96
    // token1 ≈ liquidity * sqrtPriceX96 / 2^192
    let sqrt_price_x96_f64 = sqrt_price_x96.as_u128() as f64;
    let liquidity_f64 = liquidity.as_u128() as f64;
    let two_pow_96 = 2_f64.powi(96);
    let two_pow_192 = 2_f64.powi(192);

    let token0_est = if sqrt_price_x96_f64 > 0.0 {
        liquidity_f64 / sqrt_price_x96_f64
    } else {
        0.0
    };
    let token1_est = liquidity_f64 * sqrt_price_x96_f64 / two_pow_192;

    // Convert to proper decimals
    let reserve0 = U256::from((token0_est * 10_f64.powi(token0_decimals as i32)) as u128);
    let reserve1 = U256::from((token1_est * 10_f64.powi(token1_decimals as i32)) as u128);

    println!("🔍 V3 Math Debug (conservative):");
    println!("   sqrt_price_x96: {}", sqrt_price_x96);
    println!("   liquidity: {}", liquidity);
    println!("   sqrt_price_x96_f64: {}", sqrt_price_x96_f64);
    println!("   liquidity_f64: {}", liquidity_f64);
    println!("   token0_est: {}", token0_est);
    println!("   token1_est: {}", token1_est);
    println!("   reserve0: {}", reserve0);
    println!("   reserve1: {}", reserve1);

    (reserve0, reserve1)
}

// Minimal ERC20 ABI for balanceOf
abigen!(ERC20, r#"[
    function balanceOf(address) view returns (uint256)
    function decimals() view returns (uint8)
]"#);

async fn check_v3_liquidity_debug(
    pair: &PairInfo,
    provider: &Arc<Provider<Http>>,
) -> Option<f64> {
    // Load V3 ABI
    let abi_str = fs::read_to_string("abi/uniswap_v3_pool.json").expect("Could not read V3 ABI");
    let abi: ethers::abi::Abi = serde_json::from_str(&abi_str).expect("Invalid ABI JSON");
    let contract = ethers::contract::Contract::new(
        pair.pair_address,
        abi,
        provider.clone(),
    );
    
    // Get token0 and token1 addresses
    let token0: H160 = contract.method("token0", ()).unwrap().call().await.ok()?;
    let token1: H160 = contract.method("token1", ()).unwrap().call().await.ok()?;
    
    // Get decimals for both tokens (try on-chain, fallback to pair struct)
    let token0_contract = ERC20::new(token0, provider.clone());
    let token1_contract = ERC20::new(token1, provider.clone());
    let token0_decimals = token0_contract.decimals().call().await.ok().map(|d| d as u8).or(pair.token0_decimals).unwrap_or(18);
    let token1_decimals = token1_contract.decimals().call().await.ok().map(|d| d as u8).or(pair.token1_decimals).unwrap_or(18);
    
    // Get actual balances of token0 and token1 in the pool
    let balance0 = token0_contract.balance_of(pair.pair_address).call().await.ok()?;
    let balance1 = token1_contract.balance_of(pair.pair_address).call().await.ok()?;
    
    println!("📊 V3 Pool Data (on-chain balances):");
    println!("   token0: {} (decimals: {})", format!("0x{:x}", token0), token0_decimals);
    println!("   token1: {} (decimals: {})", format!("0x{:x}", token1), token1_decimals);
    println!("   balance0: {}", balance0);
    println!("   balance1: {}", balance1);
    
    let liquidity_usd = calculate_liquidity_usd(
        balance0,
        balance1,
        &token0,
        &token1,
        token0_decimals,
        token1_decimals,
    );
    
    Some(liquidity_usd)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Debugging V3 liquidity calculation...");
    
    // Initialize provider
    let rpc_url = std::env::var("BSC_RPC_URL").unwrap_or_else(|_| {
        "http://localhost:8545/".to_string()
    });
    let provider = Arc::new(Provider::<Http>::try_from(&rpc_url)?);
    
    println!("🔗 Using RPC: {}", rpc_url);
    
    // Test specific pool address
    let test_address = "0x69B86059C5Fb3A44355937e7b505A659443b9A22";
    let test_pair = PairInfo {
        pair_address: test_address.parse::<H160>()?,
        token0: H160::zero(), // Will be fetched from contract
        token1: H160::zero(), // Will be fetched from contract
        dex_name: "Test V3 Pool".to_string(),
        dex_version: "V3".to_string(),
        token0_symbol: None,
        token1_symbol: None,
        token0_decimals: Some(18),
        token1_decimals: Some(18),
    };
    
    println!("\n📊 Testing specific V3 pool: {}", test_address);
    println!("   DEX: {}", test_pair.dex_name);
    
    if let Some(liquidity_usd) = check_v3_liquidity_debug(&test_pair, &provider).await {
        println!("✅ Liquidity: ${:.2}", liquidity_usd);
        if liquidity_usd >= 1000.0 {
            println!("✅ PASSES threshold");
        } else {
            println!("❌ FAILS threshold");
        }
    } else {
        println!("❌ Could not get liquidity");
    }
    
    // Also test with first few V3 pairs from the file for comparison
    let v3_file = std::fs::File::open("data/pairs_v3.jsonl")?;
    let v3_reader = std::io::BufReader::new(v3_file);
    
    let mut test_count = 0;
    for line in std::io::BufRead::lines(v3_reader) {
        if test_count >= 2 { break; } // Test only first 2 pairs for comparison
        
        let line = line?;
        if line.trim().is_empty() { continue; }
        
        let pair: PairInfo = serde_json::from_str(&line)?;
        
        println!("\n📊 Testing V3 pair: {}", pair.pair_address);
        println!("   DEX: {}", pair.dex_name);
        println!("   Token0: {} ({})", 
                 pair.token0_symbol.as_deref().unwrap_or("Unknown"), 
                 format!("0x{:x}", pair.token0));
        println!("   Token1: {} ({})", 
                 pair.token1_symbol.as_deref().unwrap_or("Unknown"), 
                 format!("0x{:x}", pair.token1));
        
        if let Some(liquidity_usd) = check_v3_liquidity_debug(&pair, &provider).await {
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