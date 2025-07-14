use std::collections::HashMap;
use ethers::types::Address;
use serde::{Deserialize, Serialize};

/// DEX Factory Addresses on BSC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexConfig {
    pub name: String,
    pub factory_address: Address,
    pub fee: u32, // Fee in basis points (e.g., 25 = 0.25%)
    pub version: DexVersion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DexVersion {
    V2,
    V3,
}

/// Base tokens for arbitrage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseToken {
    pub symbol: String,
    pub address: Address,
    pub decimals: u8,
    pub is_stable: bool,
}

/// Main configuration for the arbitrage bot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // DEX Configuration
    pub dexes: Vec<DexConfig>,
    
    // DEX Fee Mapping (for V2 pools)
    pub dex_fees: HashMap<String, u32>, // DEX name -> fee in basis points
    
    // Base Tokens
    pub base_tokens: Vec<BaseToken>,
    
    // Network Configuration
    pub rpc_url: String,
    pub ws_url: String,
    pub chain_id: u64,
    
    // Arbitrage Settings
    pub min_profit_threshold: u128, // Minimum profit in wei
    pub max_slippage: u32, // Maximum slippage in basis points
    pub gas_limit: u64,
    pub gas_price: u64,
    
    // Performance Settings
    pub max_parallel_workers: usize,
    pub cache_update_interval: u64, // milliseconds
    pub event_buffer_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dexes: vec![
                // PancakeSwap V2
                DexConfig {
                    name: "PancakeSwap V2".to_string(),
                    factory_address: "0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73"
                        .parse()
                        .unwrap(),
                    fee: 25, // 0.25%
                    version: DexVersion::V2,
                },
                // PancakeSwap V3
                DexConfig {
                    name: "PancakeSwap V3".to_string(),
                    factory_address: "0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865"
                        .parse()
                        .unwrap(),
                    fee: 25, // 0.25%
                    version: DexVersion::V3,
                },
                 DexConfig {
                    name: "Uniswap V3".to_string(),
                    factory_address: "0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7"
                        .parse()
                        .unwrap(),
                    fee: 25, // 0.25%
                    version: DexVersion::V3,
                },
                // BiSwap
                DexConfig {
                    name: "BiSwap".to_string(),
                    factory_address: "0x858E3312ed3A876947EA49d572A7C42DE08af7EE"
                        .parse()
                        .unwrap(),
                    fee: 10, // 0.1%
                    version: DexVersion::V2,
                },
                // ApeSwap
                DexConfig {
                    name: "ApeSwap".to_string(),
                    factory_address: "0x0841BD0B734E4F5853f0dD8d7Ea041c241fb0Da6"
                        .parse()
                        .unwrap(),
                    fee: 20, // 0.2%
                    version: DexVersion::V2,
                },
                // BakerySwap
                DexConfig {
                    name: "BakerySwap".to_string(),
                    factory_address: "0x01bF7C66c6BD861915CdaaE475042d3c4BaE16A7"
                        .parse()
                        .unwrap(),
                    fee: 30, // 0.3%
                    version: DexVersion::V2,
                },
                // MDEX
                DexConfig {
                    name: "MDEX".to_string(),
                    factory_address: "0x3CD1C46068dAEa5Ebb0d3f55F6915B10648062B8"
                        .parse()
                        .unwrap(),
                    fee: 20, // 0.2%
                    version: DexVersion::V2,
                },
                // SushiSwap BSC
                DexConfig {
                    name: "SushiSwap BSC".to_string(),
                    factory_address: "0xc35DADB65012eC5796536bD9864eD8773aBc74C4"
                        .parse()
                        .unwrap(),
                    fee: 30, // 0.3%
                    version: DexVersion::V2,
                },
            ],
            // DEX Fee Mapping for V2 pools
            dex_fees: {
                let mut fees = HashMap::new();
                fees.insert("PancakeSwap V2".to_string(), 25);    // 0.25%
                fees.insert("BiSwap".to_string(), 10);            // 0.1%
                fees.insert("ApeSwap".to_string(), 20);           // 0.2%
                fees.insert("BakerySwap".to_string(), 30);        // 0.3%
                fees.insert("MDEX".to_string(), 20);              // 0.2%
                fees.insert("SushiSwap BSC".to_string(), 30);     // 0.3%
                fees
            },
            // ["0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73", "0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865", "0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7", "0x858E3312ed3A876947EA49d572A7C42DE08af7EE", "0x0841BD0B734E4F5853f0dD8d7Ea041c241fb0Da6", "0x01bF7C66c6BD861915CdaaE475042d3c4BaE16A7", "0xc35DADB65012eC5796536bD9864eD8773aBc74C4"]
            base_tokens: vec![
                // WBNB
                BaseToken {
                    symbol: "WBNB".to_string(),
                    address: "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c"
                        .parse()
                        .unwrap(),
                    decimals: 18,
                    is_stable: false,
                },
                // BUSD
                BaseToken {
                    symbol: "BUSD".to_string(),
                    address: "0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56"
                        .parse()
                        .unwrap(),
                    decimals: 18,
                    is_stable: true,
                },
                // USDT
                BaseToken {
                    symbol: "USDT".to_string(),
                    address: "0x55d398326f99059fF775485246999027B3197955"
                        .parse()
                        .unwrap(),
                    decimals: 18,
                    is_stable: true,
                },
                // USDC
                BaseToken {
                    symbol: "USDC".to_string(),
                    address: "0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d"
                        .parse()
                        .unwrap(),
                    decimals: 18,
                    is_stable: true,
                },
                // CAKE
                BaseToken {
                    symbol: "CAKE".to_string(),
                    address: "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82"
                        .parse()
                        .unwrap(),
                    decimals: 18,
                    is_stable: false,
                },
                
                // BTCB
                BaseToken {
                    symbol: "BTCB".to_string(),
                    address: "0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c"
                        .parse()
                        .unwrap(),
                    decimals: 18,
                    is_stable: false,
                },
                //weth
                BaseToken {
                    symbol: "weth".to_string(),
                    address: "0x2170Ed0880ac9A755fd29B2688956BD959F933F8"
                        .parse()
                        .unwrap(),
                    decimals: 18,
                    is_stable: false,
                },
            ],
            
            // Local node configuration
            rpc_url: "http://127.0.0.1:8545".to_string(),
            ws_url: "ws://127.0.0.1:8546".to_string(),
            chain_id: 56,
            
            // Arbitrage Settings
            min_profit_threshold: 1000000000000000, // 0.001 BNB in wei
            max_slippage: 100, // 1%
            gas_limit: 500000,
            gas_price: 5000000000, // 5 Gwei
            
            // Performance Settings
            max_parallel_workers: num_cpus::get(),
            cache_update_interval: 100, // 100ms
            event_buffer_size: 10000,
        }
    }
}

impl Config {
    /// Get DEX by name
    pub fn get_dex_by_name(&self, name: &str) -> Option<&DexConfig> {
        self.dexes.iter().find(|dex| dex.name == name)
    }
    
    /// Get base token by symbol
    pub fn get_base_token_by_symbol(&self, symbol: &str) -> Option<&BaseToken> {
        self.base_tokens.iter().find(|token| token.symbol == symbol)
    }
    
    /// Get base token by address
    pub fn get_base_token_by_address(&self, address: Address) -> Option<&BaseToken> {
        self.base_tokens.iter().find(|token| token.address == address)
    }
    
    /// Get all V2 DEXes
    pub fn get_v2_dexes(&self) -> Vec<&DexConfig> {
        self.dexes.iter().filter(|dex| matches!(dex.version, DexVersion::V2)).collect()
    }
    
    /// Get all V3 DEXes
    pub fn get_v3_dexes(&self) -> Vec<&DexConfig> {
        self.dexes.iter().filter(|dex| matches!(dex.version, DexVersion::V3)).collect()
    }
    
    /// Get stable tokens
    pub fn get_stable_tokens(&self) -> Vec<&BaseToken> {
        self.base_tokens.iter().filter(|token| token.is_stable).collect()
    }
    
    /// Get V2 fee for a DEX by name
    pub fn get_v2_fee(&self, dex_name: &str) -> u32 {
        self.dex_fees.get(dex_name).copied().unwrap_or(25) // Default to 0.25% if not found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_default() {
        let config = Config::default();
        
        // Test DEXes
        assert!(!config.dexes.is_empty());
        assert!(config.get_dex_by_name("PancakeSwap V2").is_some());
        assert!(config.get_dex_by_name("BiSwap").is_some());
        
        // Test base tokens
        assert!(!config.base_tokens.is_empty());
        assert!(config.get_base_token_by_symbol("WBNB").is_some());
        assert!(config.get_base_token_by_symbol("USDT").is_some());
        
        // Test V2/V3 separation
        assert!(!config.get_v2_dexes().is_empty());
        assert!(!config.get_v3_dexes().is_empty());
        
        // Test stable tokens
        let stable_tokens = config.get_stable_tokens();
        assert!(!stable_tokens.is_empty());
        assert!(stable_tokens.iter().all(|token| token.is_stable));
    }
    
    #[test]
    fn test_dex_fees() {
        let config = Config::default();
        
        let pancakeswap = config.get_dex_by_name("PancakeSwap V2").unwrap();
        assert_eq!(pancakeswap.fee, 25); // 0.25%
        
        let biswap = config.get_dex_by_name("BiSwap").unwrap();
        assert_eq!(biswap.fee, 10); // 0.1%
    }
}
