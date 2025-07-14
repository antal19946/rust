use ethers::providers::{Provider, Http};
use std::sync::Arc;
use std::str::FromStr;
use ethers::types::H160;
use crate::bindings::UniswapV3Pool;

#[tokio::main]
async fn main() {
    // Test V3 pool addresses (PancakeSwap V3 pools)
    let test_pools = vec![
        "0x36696169C63e42cd08ce11f5deeBbCeBae652050", // WBNB-USDT 0.05%
        "0x4C36388bE6F416A29C8d8ED537d4B4f4C5a9E2C5", // WBNB-USDT 0.3%
        "0x7EFaEf62fDdCCa950418312c6C91Aef321375A00", // WBNB-USDT 1%
    ];
    
    let provider = Arc::new(Provider::<Http>::try_from(
        "https://bsc-dataseed1.binance.org/"
    ).unwrap());
    
    println!("🔍 Testing V3 Pool Fees:");
    println!("=========================");
    
    for pool_addr in test_pools {
        let address = H160::from_str(pool_addr).unwrap();
        let contract = UniswapV3Pool::new(address, provider.clone());
        
        match contract.fee().call().await {
            Ok(fee) => {
                println!("Pool {}: {} bps ({}%)", 
                    pool_addr, 
                    fee, 
                    fee as f64 / 100.0
                );
            }
            Err(e) => {
                println!("Pool {}: Error fetching fee - {}", pool_addr, e);
            }
        }
    }
    
    println!("\n✅ V3 fee fetching test completed!");
} 