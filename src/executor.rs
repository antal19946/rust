use ethers::types::{H160, U256};
use crate::arbitrage_finder::SimulatedRoute;
use crate::route_cache::PoolMeta;
use std::collections::HashMap;
use crate::bindings::DirectSwapExecutor;
use ethers::prelude::*;
use std::sync::Arc;
use hex;

#[derive(Debug)]
pub struct BuySellExecutionData {
    // Buy path data
    pub buy_tokens: Vec<H160>,
    pub buy_pools: Vec<H160>,
    pub buy_pool_types: Vec<u8>,
    pub buy_amounts: Vec<U256>,
    
    // Sell path data
    pub sell_tokens: Vec<H160>,
    pub sell_pools: Vec<H160>,
    pub sell_pool_types: Vec<u8>,
    pub sell_amounts: Vec<U256>,
}

impl BuySellExecutionData {
    /// Build from a SimulatedRoute, using a pool address -> PoolMeta map
    pub fn from_simulated_route(
        route: &SimulatedRoute,
        pool_meta_map: &HashMap<H160, PoolMeta>,
        token_index_map: &crate::token_index::TokenIndexMap,
    ) -> Option<Self> {
        // Convert token indices to addresses for buy path
        let buy_tokens: Vec<H160> = route.buy_path.hops.iter()
            .filter_map(|idx| token_index_map.index_to_address.get(idx).copied())
            .collect();
        if buy_tokens.len() != route.buy_path.hops.len() {
            return None;
        }

        // Convert token indices to addresses for sell path
        let sell_tokens: Vec<H160> = route.sell_path.hops.iter()
            .filter_map(|idx| token_index_map.index_to_address.get(idx).copied())
            .collect();
        if sell_tokens.len() != route.sell_path.hops.len() {
            return None;
        }

        // Build buy pool types
        let mut buy_pool_types = Vec::new();
        for pool_addr in &route.buy_pools {
            if let Some(meta) = pool_meta_map.get(pool_addr) {
                match &meta.dex_type {
                    crate::route_cache::DEXType::PancakeV3
                    | crate::route_cache::DEXType::BiSwapV3
                    | crate::route_cache::DEXType::ApeSwapV3
                    | crate::route_cache::DEXType::BakeryV3
                    | crate::route_cache::DEXType::SushiV3 => {
                        buy_pool_types.push(1u8);
                    }
                    crate::route_cache::DEXType::Other(name) if name.contains("V3") => {
                        buy_pool_types.push(1u8);
                    }
                    _ => {
                        buy_pool_types.push(0u8);
                    }
                }
            } else {
                return None;
            }
        }

        // Build sell pool types
        let mut sell_pool_types = Vec::new();
        for pool_addr in &route.sell_pools {
            if let Some(meta) = pool_meta_map.get(pool_addr) {
                match &meta.dex_type {
                    crate::route_cache::DEXType::PancakeV3
                    | crate::route_cache::DEXType::BiSwapV3
                    | crate::route_cache::DEXType::ApeSwapV3
                    | crate::route_cache::DEXType::BakeryV3
                    | crate::route_cache::DEXType::SushiV3 => {
                        sell_pool_types.push(1u8);
                    }
                    crate::route_cache::DEXType::Other(name) if name.contains("V3") => {
                        sell_pool_types.push(1u8);
                    }
                    _ => {
                        sell_pool_types.push(0u8);
                    }
                }
            } else {
                return None;
            }
        }

        Some(Self {
            buy_tokens,
            buy_pools: route.buy_pools.clone(),
            buy_pool_types,
            buy_amounts: route.buy_amounts.clone(),
            sell_tokens,
            sell_pools: route.sell_pools.clone(),
            sell_pool_types,
            sell_amounts: route.sell_amounts.clone(),
        })
    }
}

// Keep the old SwapExecutionData for backward compatibility
#[derive(Debug)]
pub struct SwapExecutionData {
    pub tokens: Vec<H160>,
    pub pools: Vec<H160>,
    pub pool_types: Vec<u8>,
    pub amounts: Vec<U256>,
    pub extra_data: Vec<Vec<u8>>,
    pub min_amount_out: U256,
}

// impl SwapExecutionData {
//     /// Build from a SimulatedRoute, using a pool address -> PoolMeta map
//     pub fn from_simulated_route(
//         route: &SimulatedRoute,
//         pool_meta_map: &HashMap<H160, PoolMeta>,
//         token_index_map: &crate::token_index::TokenIndexMap,
//         slippage_bps: u32, // e.g. 50 for 0.5%
//     ) -> Option<Self> {
//         // 1. tokens: convert merged_tokens (indices) to addresses
//         let tokens: Vec<H160> = route.merged_tokens.iter().filter_map(|idx| token_index_map.index_to_address.get(idx).copied()).collect();
//         if tokens.len() != route.merged_tokens.len() {
//             return None;
//         }
//         // 2. pools: just merged_pools
//         let pools = route.merged_pools.clone();
//         // 3. pool_types and extra_data
//         let mut pool_types = Vec::new();
//         let mut extra_data = Vec::new();
//         for pool_addr in &pools {
//             if let Some(meta) = pool_meta_map.get(pool_addr) {
//                 match &meta.dex_type {
//                     crate::route_cache::DEXType::PancakeV3
//                     | crate::route_cache::DEXType::BiSwapV3
//                     | crate::route_cache::DEXType::ApeSwapV3
//                     | crate::route_cache::DEXType::BakeryV3
//                     | crate::route_cache::DEXType::SushiV3 => {
//                         pool_types.push(1u8);
//                         let factory = meta.factory.unwrap_or_default();
//                         let fee = meta.fee.unwrap_or(2500u32);
//                         let encoded = ethers::abi::encode(&[
//                             ethers::abi::Token::Address(factory),
//                             ethers::abi::Token::Uint(fee.into()),
//                         ]);
//                         extra_data.push(encoded);
//                     }
//                     crate::route_cache::DEXType::Other(name) if name.contains("V3") => {
//                         pool_types.push(1u8);
//                         let factory = meta.factory.unwrap_or_default();
//                         let fee = meta.fee.unwrap_or(2500u32);
//                         let encoded = ethers::abi::encode(&[
//                             ethers::abi::Token::Address(factory),
//                             ethers::abi::Token::Uint(fee.into()),
//                         ]);
//                         extra_data.push(encoded);
//                     }
//                     _ => {
//                         pool_types.push(0u8);
//                         extra_data.push(vec![]);
//                     }
//                 }
//             } else {
//                 return None;
//             }
//         }
//         // 4. amounts: merged_amounts
//         let amounts = route.merged_amounts.clone();
//         // 5. min_amount_out: last amount minus slippage
//         let last = *amounts.last()?;
//         let min_amount_out = last.saturating_mul(U256::from(10_000u32 - slippage_bps)) / U256::from(10_000u32);
//         Some(Self {
//             tokens,
//             pools,
//             pool_types,
//             amounts,
//             extra_data,
//             min_amount_out,
//         })
//     }
// }

pub async fn execute_arbitrage_onchain(
    contract_address: H160,
    swap_data: BuySellExecutionData,
    wallet: LocalWallet,
    provider: Arc<Provider<Http>>,
) -> Result<TxHash, Box<dyn std::error::Error>> {
    let client = SignerMiddleware::new(provider.clone(), wallet.clone());
    let client = Arc::new(client);
    let contract = DirectSwapExecutor::new(contract_address, client.clone());

    // --- Dynamic Gas (EIP-1559 preferred, fallback to legacy) ---
    let block = provider.get_block(BlockNumber::Pending).await?.unwrap();
    let base_fee = block.base_fee_per_gas.unwrap_or(U256::from(0));
    let priority_fee = U256::from(100_000_000u64); // 2 gwei
    let max_fee_per_gas = base_fee + priority_fee;
    println!("[EXECUTOR] Using base_fee: {} priority_fee: {} max_fee_per_gas: {}", base_fee, priority_fee, max_fee_per_gas);

    // --- Current Nonce ---
    let nonce = provider.get_transaction_count(wallet.address(), None).await?;
    println!("[EXECUTOR] Using nonce: {:?}", nonce);

    // --- Simulate call (dry run) ---
    let call = contract.buy_sell_execution(
        swap_data.buy_tokens.clone(),
        swap_data.buy_pools.clone(),
        swap_data.buy_pool_types.clone(),
        swap_data.buy_amounts.clone(),
        swap_data.sell_tokens.clone(),
        swap_data.sell_pools.clone(),
        swap_data.sell_pool_types.clone(),
        swap_data.sell_amounts.clone(),
    );
    let simulation = call.clone().call().await;
    match simulation {
        Ok(_) => println!("[EXECUTOR] Simulation succeeded!"),
        Err(e) => {
            println!("[EXECUTOR] Simulation failed: {:?}", e);
            return Err(format!("Simulation failed: {e:?}").into());
        }
    }

    // --- Send TX with dynamic gas ---
    let call_with_opts = call
        .gas_price(max_fee_per_gas)
        .gas(400_000u64)
        .nonce(nonce);

    let pending_tx = call_with_opts.send().await?;

    let tx_hash = pending_tx.tx_hash();
    println!("[EXECUTOR] TX fired: https://bscscan.com/tx/{:?}", tx_hash);

    let receipt = pending_tx.await?;
    if let Some(receipt) = &receipt {
        if receipt.status == Some(U64::from(1u64)) {
            println!("[EXECUTOR] TX succeeded! Hash: {:?}", receipt.transaction_hash);
            Ok(receipt.transaction_hash)
        } else {
            println!("[EXECUTOR] TX failed! Hash: {:?}", receipt.transaction_hash);
            Err("Transaction failed on-chain".into())
        }
    } else {
        println!("[EXECUTOR] No transaction receipt returned! Hash: {:?}", tx_hash);
        Err("No transaction receipt returned".into())
    }
}

// Keep the old function for backward compatibility
pub async fn execute_arbitrage_onchain_legacy(
    contract_address: H160,
    swap_data: SwapExecutionData,
    wallet: LocalWallet,
    provider: Arc<Provider<Http>>,
) -> Result<TxHash, Box<dyn std::error::Error>> {
    let client = SignerMiddleware::new(provider.clone(), wallet.clone());
    let client = Arc::new(client);
    let contract = DirectSwapExecutor::new(contract_address, client.clone());
    let extra_data_bytes: Vec<ethers::types::Bytes> = swap_data.extra_data.into_iter().map(ethers::types::Bytes::from).collect();

    // --- Dynamic Gas (EIP-1559 preferred, fallback to legacy) ---
    let block = provider.get_block(BlockNumber::Pending).await?.unwrap();
    let base_fee = block.base_fee_per_gas.unwrap_or(U256::from(0));
    let priority_fee = U256::from(100_000_000u64); // 2 gwei
    let max_fee_per_gas = base_fee + priority_fee;
    println!("[EXECUTOR] Using base_fee: {} priority_fee: {} max_fee_per_gas: {}", base_fee, priority_fee, max_fee_per_gas);

    // --- Current Nonce ---
    let nonce = provider.get_transaction_count(wallet.address(), None).await?;
    println!("[EXECUTOR] Using nonce: {:?}", nonce);

    // --- Simulate call (dry run) ---
    let call = contract.execute_swap(
        swap_data.tokens.clone(),
        swap_data.pools.clone(),
        swap_data.pool_types.clone(),
        swap_data.amounts.clone(),
        extra_data_bytes.clone(),
        swap_data.min_amount_out,
    );
    let simulation = call.clone().call().await;
    match simulation {
        Ok(_) => println!("[EXECUTOR] Simulation succeeded!"),
        Err(e) => {
            println!("[EXECUTOR] Simulation failed: {:?}", e);
            return Err(format!("Simulation failed: {e:?}").into());
        }
    }

    // --- Send TX with dynamic gas ---
    let call_with_opts = call
        .gas_price(max_fee_per_gas)
        .gas(400_000u64)
        .nonce(nonce);

    let pending_tx = call_with_opts.send().await?;

    let tx_hash = pending_tx.tx_hash();
    println!("[EXECUTOR] TX fired: https://bscscan.com/tx/{:?}", tx_hash);

    let receipt = pending_tx.await?;
    if let Some(receipt) = &receipt {
        if receipt.status == Some(U64::from(1u64)) {
            println!("[EXECUTOR] TX succeeded! Hash: {:?}", receipt.transaction_hash);
            Ok(receipt.transaction_hash)
        } else {
            println!("[EXECUTOR] TX failed! Hash: {:?}", receipt.transaction_hash);
            Err("Transaction failed on-chain".into())
        }
    } else {
        println!("[EXECUTOR] No transaction receipt returned! Hash: {:?}", tx_hash);
        Err("No transaction receipt returned".into())
    }
}

/// Decode a Solidity revert reason (Error(string)) from hex revert data
pub fn decode_revert_reason(data: &str) -> Option<String> {
    let data = data.strip_prefix("0x").unwrap_or(data);
    if data.starts_with("08c379a0") && data.len() > 8 + 64 {
        let reason_start = 8 + 64 + 64;
        let len_hex = &data[8+64..8+64+64];
        let len = usize::from_str_radix(len_hex, 16).unwrap_or(0) * 2;
        let reason_hex = &data[reason_start..reason_start+len.min(data.len()-reason_start)];
        if let Ok(bytes) = hex::decode(reason_hex) {
            if let Ok(reason) = String::from_utf8(bytes) {
                return Some(reason);
            }
        }
    }
    None
}
