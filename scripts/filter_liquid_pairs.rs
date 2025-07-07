use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::collections::HashMap;
use ethers::types::{H160, U256};
use ethers::providers::{Provider, Http};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

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

// Known token addresses with USD values (for liquidity calculation)
const KNOWN_TOKENS: &[(&str, &str, f64)] = &[
    // BNB
    ("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", "BNB", 300.0),
    // USDT
    ("0x55d398326f99059fF775485246999027B3197955", "USDT", 1.0),
    // USDC
    ("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d", "USDC", 1.0),
    // BUSD
    ("0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56", "BUSD", 1.0),
    // WBNB
    ("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", "WBNB", 300.0),
    // CAKE
    ("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82", "CAKE", 2.0),
    // ETH
    ("0x2170Ed0880ac9A755fd29B2688956BD959F933F8", "ETH", 2000.0),
    // BTC
    ("0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c", "BTC", 40000.0),
    // ADA
    ("0x3EE2200Efb3400fAbB9AacF31297cBdD1d435D47", "ADA", 0.5),
    // DOT
    ("0x7083609fCE4d1d8Dc0C979AAb8c869Ea2C873402", "DOT", 7.0),
    // LINK
    ("0xF8A0BF9cF54Bb92F17374d9e9A321E6a111a51bD", "LINK", 15.0),
    // LTC
    ("0x4338665CBB7B2485A8855A139b75D5e34AB0DB94", "LTC", 80.0),
    // BCH
    ("0x8fF795a6F4D97E7887C79beA79aba5cc76444aDf", "BCH", 400.0),
    // XRP
    ("0x1D2F0da169ceB9fC7B3144628dB156f3F6c60dBE", "XRP", 0.6),
    // EOS
    ("0x56b6fB708fC5732DEC1Afc8D8556423A2EDcCbD6", "EOS", 0.8),
    // TRX
    ("0x85EAC5Ac2F758618dFa09bDbe0cf174e7d574D5B", "TRX", 0.1),
    // MATIC
    ("0xCC42724C6683B7E57334c4E856f4c9965ED682bD", "MATIC", 0.8),
    // AVAX
    ("0x1CE0c2827e2eF14D5C4f29a091d735A204794041", "AVAX", 35.0),
    // SOL
    ("0x570A5D26f7765Ecb712C0924E4De545B89fD43dF", "SOL", 100.0),
    // FTM
    ("0xAD29AbB318791D579433D831ed122aFeAf29dcfe", "FTM", 0.3),
    // NEAR
    ("0x1Fa4a73a3F0133f0025378af00236f3aBDEE5D63", "NEAR", 5.0),
    // ALGO
    ("0xa1faa113cbE53436Df28FF0aEe54275c13B40975", "ALGO", 0.2),
    // ATOM
    ("0x0Eb3a705fc54725037CC9e008bDede697f62F335", "ATOM", 8.0),
    // UNI
    ("0xBf5140A22578168FD562DCcF235E5D43A02ce9B1", "UNI", 7.0),
    // AAVE
    ("0xfb6115445Bff7b52FeB98650C87f44907E58f802", "AAVE", 100.0),
    // COMP
    ("0x52CE071Bd9b1C4B00A0b92D298c512478CaD67e8", "COMP", 60.0),
    // MKR
    ("0x5f0Da599BB2ccCfcf6Fdfd7D81743B6020864350", "MKR", 2000.0),
    // YFI
    ("0x88f1A5ae2A3BF98AEAF342D26B30a79438c9142e", "YFI", 8000.0),
    // CRV
    ("0x96Dd399F9c3AFda1F194182F716eF599a2952f39", "CRV", 0.5),
    // SUSHI
    ("0x947950BcC74888a40Ffa2593C5798F11Fc9124C4", "SUSHI", 1.0),
    // 1INCH
    ("0x111111111117dC0aa78b770fA6A738034120C302", "1INCH", 0.4),
];

fn get_token_usd_value(token_address: &H160) -> Option<f64> {
    let addr_str = format!("0x{:x}", token_address);
    KNOWN_TOKENS.iter()
        .find(|(addr, _, _)| *addr == addr_str)
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
    use crate::bindings::UniswapV2Pair;
    
    let contract = UniswapV2Pair::new(pair.pair_address, provider.clone());
    
    match contract.get_reserves().call().await {
        Ok((reserve0, reserve1, _)) => {
            let token0_decimals = pair.token0_decimals.unwrap_or(18);
            let token1_decimals = pair.token1_decimals.unwrap_or(18);
            
            let liquidity_usd = calculate_liquidity_usd(
                reserve0.into(),
                reserve1.into(),
                &pair.token0,
                &pair.token1,
                token0_decimals,
                token1_decimals,
            );
            
            if liquidity_usd >= 1000.0 {
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
                    reserve0: reserve0.into(),
                    reserve1: reserve1.into(),
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

async fn check_v3_liquidity(
    pair: &PairInfo,
    provider: &Arc<Provider<Http>>,
) -> Option<LiquidPairInfo> {
    use crate::bindings::UniswapV3Pool;
    
    let contract = UniswapV3Pool::new(pair.pair_address, provider.clone());
    
    // Get slot0 and liquidity
    let slot0_future = contract.slot_0().call();
    let liquidity_future = contract.liquidity().call();
    
    match tokio::join!(slot0_future, liquidity_future) {
        (Ok(slot0), Ok(liquidity)) => {
            let sqrt_price_x96 = slot0.0;
            let tick = slot0.1;
            let liquidity_amount = liquidity;
            
            // Calculate reserves from sqrt_price_x96 and liquidity
            // This is a simplified calculation - for exact reserves we'd need more complex V3 math
            let price = (sqrt_price_x96.as_u128() as f64 / 2_f64.powi(96)).powi(2);
            
            let token0_decimals = pair.token0_decimals.unwrap_or(18);
            let token1_decimals = pair.token1_decimals.unwrap_or(18);
            
            // Estimate reserves from liquidity and price
            let liquidity_f64 = liquidity_amount.as_u128() as f64 / 10_f64.powi(18);
            let reserve0_estimate = liquidity_f64 * price.sqrt();
            let reserve1_estimate = liquidity_f64 / price.sqrt();
            
            let reserve0 = U256::from((reserve0_estimate * 10_f64.powi(token0_decimals as i32)) as u128);
            let reserve1 = U256::from((reserve1_estimate * 10_f64.powi(token1_decimals as i32)) as u128);
            
            let liquidity_usd = calculate_liquidity_usd(
                reserve0,
                reserve1,
                &pair.token0,
                &pair.token1,
                token0_decimals,
                token1_decimals,
            );
            
            if liquidity_usd >= 1000.0 {
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
            eprintln!("Error checking V3 liquidity for {}: {:?}", pair.pair_address, e);
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Starting liquidity-based pair filtering...");
    
    // Initialize provider
    let rpc_url = std::env::var("BSC_RPC_URL").unwrap_or_else(|_| {
        "https://bsc-dataseed1.binance.org/".to_string()
    });
    let provider = Arc::new(Provider::<Http>::try_from(rpc_url)?);
    
    let min_liquidity_usd = 1000.0;
    println!("📊 Minimum liquidity threshold: ${}", min_liquidity_usd);
    
    let mut total_v2_pairs = 0;
    let mut total_v3_pairs = 0;
    let mut liquid_v2_pairs = 0;
    let mut liquid_v3_pairs = 0;
    
    // Process V2 pairs
    println!("🔍 Processing V2 pairs...");
    let v2_file = File::open("data/pairs_v2.jsonl")?;
    let v2_reader = BufReader::new(v2_file);
    let mut v2_output = BufWriter::new(File::create("data/liquid_pairs_v2.jsonl")?);
    
    for line in v2_reader.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        
        total_v2_pairs += 1;
        let pair: PairInfo = serde_json::from_str(&line)?;
        
        if let Some(liquid_pair) = check_v2_liquidity(&pair, &provider).await {
            liquid_v2_pairs += 1;
            let json = serde_json::to_string(&liquid_pair)?;
            writeln!(v2_output, "{}", json)?;
            
            if liquid_v2_pairs % 100 == 0 {
                println!("✅ V2: Found {} liquid pairs out of {} processed", liquid_v2_pairs, total_v2_pairs);
            }
        }
        
        // Rate limiting
        if total_v2_pairs % 50 == 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
    
    // Process V3 pairs
    println!("🔍 Processing V3 pairs...");
    let v3_file = File::open("data/pairs_v3.jsonl")?;
    let v3_reader = BufReader::new(v3_file);
    let mut v3_output = BufWriter::new(File::create("data/liquid_pairs_v3.jsonl")?);
    
    for line in v3_reader.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        
        total_v3_pairs += 1;
        let pair: PairInfo = serde_json::from_str(&line)?;
        
        if let Some(liquid_pair) = check_v3_liquidity(&pair, &provider).await {
            liquid_v3_pairs += 1;
            let json = serde_json::to_string(&liquid_pair)?;
            writeln!(v3_output, "{}", json)?;
            
            if liquid_v3_pairs % 50 == 0 {
                println!("✅ V3: Found {} liquid pairs out of {} processed", liquid_v3_pairs, total_v3_pairs);
            }
        }
        
        // Rate limiting
        if total_v3_pairs % 50 == 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
    
    // Create combined file
    println!("📝 Creating combined liquid pairs file...");
    let mut combined_output = BufWriter::new(File::create("data/liquid_pairs_combined.jsonl")?);
    
    // Read and combine both files
    let v2_liquid_file = File::open("data/liquid_pairs_v2.jsonl")?;
    let v2_liquid_reader = BufReader::new(v2_liquid_file);
    for line in v2_liquid_reader.lines() {
        writeln!(combined_output, "{}", line?)?;
    }
    
    let v3_liquid_file = File::open("data/liquid_pairs_v3.jsonl")?;
    let v3_liquid_reader = BufReader::new(v3_liquid_file);
    for line in v3_liquid_reader.lines() {
        writeln!(combined_output, "{}", line?)?;
    }
    
    println!("🎉 Filtering completed!");
    println!("📊 Results:");
    println!("   V2 pairs: {} total → {} liquid ({}%)", 
             total_v2_pairs, liquid_v2_pairs, 
             (liquid_v2_pairs as f64 / total_v2_pairs as f64 * 100.0) as i32);
    println!("   V3 pairs: {} total → {} liquid ({}%)", 
             total_v3_pairs, liquid_v3_pairs,
             (liquid_v3_pairs as f64 / total_v3_pairs as f64 * 100.0) as i32);
    println!("   Total liquid pairs: {}", liquid_v2_pairs + liquid_v3_pairs);
    println!("📁 Output files:");
    println!("   - data/liquid_pairs_v2.jsonl");
    println!("   - data/liquid_pairs_v3.jsonl");
    println!("   - data/liquid_pairs_combined.jsonl");
    
    Ok(())
} 