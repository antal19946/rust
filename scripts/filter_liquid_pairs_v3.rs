use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use ethers::types::{H160, U256};
use ethers::providers::{Provider, Http};
use ethers::contract::abigen;
use serde::{Deserialize, Serialize};
use futures::stream::{self, StreamExt};

// Minimal ERC20 ABI for balanceOf
abigen!(ERC20, r#"[
    function balanceOf(address) view returns (uint256)
    function decimals() view returns (uint8)
]"#);

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

// Known token addresses with USD values (2025) - Updated prices
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

fn u256_to_f64_lossy(val: &U256) -> f64 {
    if val.bits() <= 128 {
        val.as_u128() as f64
    } else {
        val.to_string().parse::<f64>().unwrap_or(f64::MAX)
    }
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
    let reserve0_f64 = u256_to_f64_lossy(&reserve0) / 10_f64.powi(token0_decimals as i32);
    let reserve1_f64 = u256_to_f64_lossy(&reserve1) / 10_f64.powi(token1_decimals as i32);
    let liquidity0_usd = reserve0_f64 * price0;
    let liquidity1_usd = reserve1_f64 * price1;
    liquidity0_usd + liquidity1_usd
}

async fn check_v3_liquidity(
    pair: &PairInfo,
    provider: &Arc<Provider<Http>>,
) -> Option<f64> {
    let token0_contract = ERC20::new(pair.token0, provider.clone());
    let token1_contract = ERC20::new(pair.token1, provider.clone());
    let token0_decimals = token0_contract.decimals().call().await.ok().map(|d| d as u8).or(pair.token0_decimals).unwrap_or(18);
    let token1_decimals = token1_contract.decimals().call().await.ok().map(|d| d as u8).or(pair.token1_decimals).unwrap_or(18);
    let balance0 = token0_contract.balance_of(pair.pair_address).call().await.ok()?;
    let balance1 = token1_contract.balance_of(pair.pair_address).call().await.ok()?;
    Some(calculate_liquidity_usd(
        balance0,
        balance1,
        &pair.token0,
        &pair.token1,
        token0_decimals,
        token1_decimals,
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc_url = std::env::var("BSC_RPC_URL").unwrap_or_else(|_| "http://localhost:8545/".to_string());
    let provider = Arc::new(Provider::<Http>::try_from(&rpc_url)?);
    let input_file = "data/pairs_v3.jsonl";
    let output_file = "data/liquid_pairs_v3_new.jsonl";
    let min_liquidity_usd = 9.0;
    let max_concurrent = 32;
    let reader = BufReader::new(File::open(input_file)?);
    let pairs: Vec<PairInfo> = reader.lines().filter_map(|l| l.ok()).filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    println!("[V3 FILTER] Total pairs: {}", pairs.len());
    
    let mut writer = File::create(output_file)?;
    let mut count_total = 0;
    let mut count_liquid = 0;
    
    for pair in pairs {
        count_total += 1;
        if let Some(liquidity_usd) = check_v3_liquidity(&pair, &provider).await {
            if liquidity_usd >= min_liquidity_usd {
                let line = serde_json::to_string(&pair).unwrap();
                let _ = writeln!(&mut writer, "{}", line);
                println!("[LIQUID] {}: ${:.2}", pair.pair_address, liquidity_usd);
                count_liquid += 1;
            } else {
                println!("[ILLQUID] {}: ${:.2}", pair.pair_address, liquidity_usd);
            }
        } else {
            println!("[ERROR] {}", pair.pair_address);
        }
    }
    
    println!("[V3 FILTER] Liquid pairs: {} / {}", count_liquid, count_total);
    Ok(())
} 