use std::fs::File;
use std::io::{BufRead, BufReader};
use serde::{Deserialize, Serialize};
use ethers::types::{H160, U256};
use ethers::providers::{Provider, Http};
use std::sync::Arc;
use std::str::FromStr;

#[derive(Debug, Deserialize, Serialize, Clone)]
struct PairInfo {
    pair_address: String,
    token0: String,
    token1: String,
    dex_name: String,
    dex_version: String,
    token0_symbol: Option<String>,
    token1_symbol: Option<String>,
    token0_decimals: Option<u8>,
    token1_decimals: Option<u8>,
}

// Known liquid tokens (major tokens that are likely to have good liquidity)
const KNOWN_LIQUID_TOKENS: &[&str] = &[
    // Major tokens
    "BNB", "WBNB", "USDT", "USDC", "BUSD", "CAKE", "ETH", "BTC", "ADA", "DOT",
    "LINK", "LTC", "BCH", "XRP", "EOS", "TRX", "XLM", "VET", "MATIC", "AVAX",
    "SOL", "FTM", "NEAR", "ALGO", "ATOM", "UNI", "AAVE", "COMP", "MKR", "YFI",
    "CRV", "SUSHI", "1INCH", "BAL", "REN", "KNC", "ZRX", "BAT", "REP", "ZEC",
    "DASH", "XMR", "XTZ", "NEO", "ONT", "QTUM", "IOTA", "ICX", "WAVES", "OMG",
];

// Known liquid token addresses (BSC)
const KNOWN_LIQUID_ADDRESSES: &[&str] = &[
    // BNB
    "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c",
    // USDT
    "0x55d398326f99059fF775485246999027B3197955",
    // USDC
    "0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d",
    // BUSD
    "0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56",
    // CAKE
    "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82",
    // ETH
    "0x2170Ed0880ac9A755fd29B2688956BD959F933F8",
    // BTC
    "0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c",
];

fn is_likely_liquid_pair_simple(pair: &PairInfo) -> bool {
    // Check if either token is in our known liquid tokens list
    let token0_symbol = pair.token0_symbol.as_deref().unwrap_or("").to_uppercase();
    let token1_symbol = pair.token1_symbol.as_deref().unwrap_or("").to_uppercase();
    
    // Check symbols
    let has_liquid_symbol = KNOWN_LIQUID_TOKENS.iter().any(|&token| {
        token0_symbol == token || token1_symbol == token
    });
    
    // Check addresses
    let has_liquid_address = KNOWN_LIQUID_ADDRESSES.iter().any(|&addr| {
        pair.token0.to_lowercase() == addr.to_lowercase() || 
        pair.token1.to_lowercase() == addr.to_lowercase()
    });
    
    // Additional heuristics
    let has_usdt_usdc_busd = token0_symbol.contains("USDT") || token0_symbol.contains("USDC") || token0_symbol.contains("BUSD") ||
                             token1_symbol.contains("USDT") || token1_symbol.contains("USDC") || token1_symbol.contains("BUSD");
    
    let has_bnb = token0_symbol.contains("BNB") || token1_symbol.contains("BNB") ||
                  pair.token0.to_lowercase() == "0xbb4cdb9cbd36b01bd1cbaeff2de08d9173bc095c" ||
                  pair.token1.to_lowercase() == "0xbb4cdb9cbd36b01bd1cbaeff2de08d9173bc095c";
    
    // Check for major DEXes (more likely to have liquid pairs)
    let is_major_dex = matches!(pair.dex_name.as_str(), 
        "PancakeSwap" | "BiSwap" | "ApeSwap" | "BakerySwap" | "SushiSwap" | "MDEX" | "DODO" | "Curve"
    );
    
    // Return true if any condition is met
    has_liquid_symbol || has_liquid_address || has_usdt_usdc_busd || has_bnb || is_major_dex
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Testing liquidity filtering logic...");
    
    // Test the specific pair mentioned by user
    let test_pair = PairInfo {
        pair_address: "0x157f21C93384df6b0D6526DF045397b11828Dd51".to_string(),
        token0: "0x1234567890123456789012345678901234567890".to_string(),
        token1: "0x0987654321098765432109876543210987654321".to_string(),
        dex_name: "PancakeSwap".to_string(),
        dex_version: "V2".to_string(),
        token0_symbol: Some("TEST1".to_string()),
        token1_symbol: Some("TEST2".to_string()),
        token0_decimals: Some(18),
        token1_decimals: Some(18),
    };
    
    println!("📊 Testing pair: {}", test_pair.pair_address);
    println!("   DEX: {}", test_pair.dex_name);
    println!("   Token0: {} ({})", test_pair.token0_symbol.as_deref().unwrap_or("Unknown"), test_pair.token0);
    println!("   Token1: {} ({})", test_pair.token1_symbol.as_deref().unwrap_or("Unknown"), test_pair.token1);
    
    let simple_result = is_likely_liquid_pair_simple(&test_pair);
    println!("✅ Simple filtering result: {}", simple_result);
    
    if simple_result {
        println!("❌ PROBLEM: Simple filtering passed this pair even though it has low liquidity!");
        println!("   This is because it's on PancakeSwap (major DEX)");
        println!("   Simple filtering only checks DEX names, not actual liquidity");
    }
    
    // Now let's check our actual filtered files
    println!("\n🔍 Checking if this pair exists in our filtered files...");
    
    let mut found_in_v2 = false;
    let mut found_in_v3 = false;
    let mut found_in_combined = false;
    
    // Check V2 file
    if let Ok(file) = File::open("data/liquid_pairs_v2.jsonl") {
        let reader = BufReader::new(file);
        for line in reader.lines() {
            if let Ok(line) = line {
                if line.contains(&test_pair.pair_address) {
                    found_in_v2 = true;
                    break;
                }
            }
        }
    }
    
    // Check V3 file
    if let Ok(file) = File::open("data/liquid_pairs_v3.jsonl") {
        let reader = BufReader::new(file);
        for line in reader.lines() {
            if let Ok(line) = line {
                if line.contains(&test_pair.pair_address) {
                    found_in_v3 = true;
                    break;
                }
            }
        }
    }
    
    // Check combined file
    if let Ok(file) = File::open("data/liquid_pairs_combined.jsonl") {
        let reader = BufReader::new(file);
        for line in reader.lines() {
            if let Ok(line) = line {
                if line.contains(&test_pair.pair_address) {
                    found_in_combined = true;
                    break;
                }
            }
        }
    }
    
    println!("📁 File check results:");
    println!("   V2 file: {}", if found_in_v2 { "FOUND" } else { "NOT FOUND" });
    println!("   V3 file: {}", if found_in_v3 { "FOUND" } else { "NOT FOUND" });
    println!("   Combined file: {}", if found_in_combined { "FOUND" } else { "NOT FOUND" });
    
    if !found_in_v2 && !found_in_v3 && !found_in_combined {
        println!("✅ GOOD: This pair is NOT in our filtered files");
        println!("   This means it was correctly filtered out");
    } else {
        println!("❌ PROBLEM: This pair IS in our filtered files");
        println!("   This confirms the simple filtering is not accurate");
    }
    
    println!("\n💡 CONCLUSION:");
    println!("   Simple filtering is heuristic-based and not accurate");
    println!("   It passes pairs based on DEX names, not actual liquidity");
    println!("   For accurate filtering, we need on-chain liquidity checks");
    println!("   But that would be much slower (RPC calls for each pair)");
    
    Ok(())
} 