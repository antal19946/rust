use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::collections::HashSet;
use serde::{Deserialize, Serialize};

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
    // BSC specific
    "WBNB", "BUSD", "USDT", "USDC", "CAKE", "BAKE", "ALPACA", "BIFI", "BUNNY",
    "DEGO", "EPS", "FARM", "FINE", "HARD", "HELMET", "JUL", "KAVA", "KLAY",
    "LINA", "LIT", "LPT", "LQTY", "LUSD", "MASK", "MBOX", "MIM", "MKR", "MLS",
    "MULTI", "NULS", "OXT", "PAXG", "PEOPLE", "PERP", "POND", "PUNDIX", "QI",
    "QUICK", "RAD", "RARE", "RARI", "REN", "REQ", "RLC", "ROSE", "RSR", "RUNE",
    "SAND", "SHIB", "SKL", "SLP", "SNX", "SPELL", "STX", "SUPER", "SUSHI",
    "SWAP", "SXP", "THETA", "TLM", "TOKE", "TRIBE", "TRU", "UMA", "UNI", "USDP",
    "UST", "VRA", "WOO", "XEC", "XRP", "XTZ", "YFI", "YGG", "ZEN", "ZIL", "ZRX",
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
    // ADA
    "0x3EE2200Efb3400fAbB9AacF31297cBdD1d435D47",
    // DOT
    "0x7083609fCE4d1d8Dc0C979AAb8c869Ea2C873402",
    // LINK
    "0xF8A0BF9cF54Bb92F17374d9e9A321E6a111a51bD",
    // LTC
    "0x4338665CBB7B2485A8855A139b75D5e34AB0DB94",
    // BCH
    "0x8fF795a6F4D97E7887C79beA79aba5cc76444aDf",
    // XRP
    "0x1D2F0da169ceB9fC7B3144628dB156f3F6c60dBE",
    // EOS
    "0x56b6fB708fC5732DEC1Afc8D8556423A2EDcCbD6",
    // TRX
    "0x85EAC5Ac2F758618dFa09bDbe0cf174e7d574D5B",
    // XLM
    "0x43cFc3b74d81acbeb4f5a064c9ab3b5c8b5c8b5c",
    // VET
    "0x6FDcdfef7c496407cCb0cEC90f9C5Aaa1Cc8D888",
    // MATIC
    "0xCC42724C6683B7E57334c4E856f4c9965ED682bD",
    // AVAX
    "0x1CE0c2827e2eF14D5C4f29a091d735A204794041",
    // SOL
    "0x570A5D26f7765Ecb712C0924E4De545B89fD43dF",
    // FTM
    "0xAD29AbB318791D579433D831ed122aFeAf29dcfe",
    // NEAR
    "0x1Fa4a73a3F0133f0025378af00236f3aBDEE5D63",
    // ALGO
    "0xa1faa113cbE53436Df28FF0aEe54275c13B40975",
    // ATOM
    "0x0Eb3a705fc54725037CC9e008bDede697f62F335",
    // UNI
    "0xBf5140A22578168FD562DCcF235E5D43A02ce9B1",
    // AAVE
    "0xfb6115445Bff7b52FeB98650C87f44907E58f802",
    // COMP
    "0x52CE071Bd9b1C4B00A0b92D298c512478CaD67e8",
    // MKR
    "0x5f0Da599BB2ccCfcf6Fdfd7D81743B6020864350",
    // YFI
    "0x88f1A5ae2A3BF98AEAF342D26B30a79438c9142e",
    // CRV
    "0x96Dd399F9c3AFda1F194182F716eF599a2952f39",
    // SUSHI
    "0x947950BcC74888a40Ffa2593C5798F11Fc9124C4",
    // 1INCH
    "0x111111111117dC0aa78b770fA6A738034120C302",
    // BAL
    "0xE48EBd2E5c8b8C8c8c8c8c8c8c8c8c8c8c8c8c8c8",
    // REN
    "0xEA3C7383b9Bc4ac15fcdadCE07e2E25Dc8a5Fc2c",
    // KNC
    "0xfe56d5892BDffc7BF58f2E84BE1b2C31D4937c56",
    // ZRX
    "0xB4699C62102037003151cd0c3a1654A9AE7D3e42",
    // BAT
    "0x101d82428437127bF1608F699CD651e6Abf9766E",
    // REP
    "0x6982508145454Ce325dDbE47a25d4ec3d2311933",
    // ZEC
    "0x1Ba42e5193dfA8B03D15dd1B86a3113bbBEF8Eeb",
    // DASH
    "0x154A9F9cbd3449AD22FDaE23044319D6eF2a1Fab",
    // XMR
    "0x465a5a630482f3abD6d3b84B39b29b07214d19e5",
    // XTZ
    "0x16939ef78684453bfDFb47825F8a5F714f12623",
    // NEO
    "0xFb4c0e4Ee7Ecef5f07C8C755c56B7e0C8c1C1C1C1",
    // ONT
    "0xFd7B3A77848f1C2D67E05E54d78d174a0C850335",
    // QTUM
    "0x9Bdc5f6C80b556e461655191c0Fb4E2c79B8d8C6",
    // IOTA
    "0xd944f1D1e9d5f9Bb90b62f9D45e447D17A60A0f5",
    // ICX
    "0x9bdc5f6c80b556e461655191c0fb4e2c79b8d8c6",
    // WAVES
    "0x1f9f6a696c6bd109bc6d0293d2e429d7d05f0e0a",
    // OMG
    "0x6b3595068778dd592e39a122f4f5a5cf09c90fe2",
];

fn is_likely_liquid_pair(pair: &PairInfo) -> bool {
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
    println!("🚀 Starting simple liquidity-based pair filtering...");
    
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
        
        if is_likely_liquid_pair(&pair) {
            liquid_v2_pairs += 1;
            writeln!(v2_output, "{}", line)?;
            
            if liquid_v2_pairs % 1000 == 0 {
                println!("✅ V2: Found {} liquid pairs out of {} processed", liquid_v2_pairs, total_v2_pairs);
            }
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
        
        if is_likely_liquid_pair(&pair) {
            liquid_v3_pairs += 1;
            writeln!(v3_output, "{}", line)?;
            
            if liquid_v3_pairs % 500 == 0 {
                println!("✅ V3: Found {} liquid pairs out of {} processed", liquid_v3_pairs, total_v3_pairs);
            }
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