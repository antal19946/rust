// utils.rs

// use primitive_types::U256;
// use std::f64::consts::E;
use revm::primitives::{Address as RevmAddress, U256, Bytes, TxKind};
use revm::context::TxEnv;
use ethers::types::Transaction; // Removed NameOrAddress as it's not directly used in the match pattern

/// Converts an ethers::types::Transaction to a revm::context::TxEnv.
///
/// This function takes a transaction object from the ethers-rs library
/// and transforms it into a TxEnv object, which is required by the
/// revm (Rust EVM) for transaction execution. It handles the mapping
/// of various transaction fields and type conversions between the two libraries.
pub fn ethers_tx_to_revm_txenv(tx: &Transaction) -> TxEnv {
    // 1. Determine the transaction kind (TxKind)
    //    If `tx.to` is an address, it's a Call. Otherwise (if `tx.to` is None), it's a Create.
    let kind = match tx.to {
        Some(addr) => TxKind::Call(RevmAddress::from(addr.0)), // Use addr.0 for raw bytes
        None => TxKind::Create, // No 'to' address means contract creation
    };

    // 2. Extract and convert optional fields, passing them as Option<T> where revm expects it.
    //    For fields like gas_price and nonce which are non-optional in revm TxEnv's builder,
    //    we provide a default if the ethers field is None.
    let chain_id = tx.chain_id.map(|id| id.as_u64()); // This is already Option<u64>
    let gas_price = tx.gas_price.map(|g| g.as_u128()).unwrap_or_default(); // Convert to u128, provide default 0 if None
    let gas_priority_fee = tx.max_priority_fee_per_gas.map(|g| g.as_u128()); // This is already Option<u128>
    let nonce = tx.nonce.as_u64(); // Nonce is usually not optional in ethers::types::Transaction

    // 3. Build the TxEnv object using the builder pattern
    let builder = TxEnv::builder()
        .caller(RevmAddress::from(tx.from.0)) // The address sending the transaction
        .kind(kind) // Type of transaction: Call (to an address) or Create (new contract)
        .value(U256::from_limbs(tx.value.0)) // Amount of Ether to send
        .data(Bytes::copy_from_slice(&tx.input.0)) // Input data for contract call or contract bytecode
        .gas_limit(tx.gas.as_u64()) // Maximum gas allowed for the transaction
        .chain_id(chain_id) // Pass Option<u64> directly
        .gas_price(gas_price) // Pass u128 directly (unwrapped with default)
        .gas_priority_fee(gas_priority_fee) // Pass Option<u128> directly
        .nonce(nonce); // Pass u64 directly (nonce is not optional in ethers Transaction)

    // 4. Finalize the TxEnv object
    //    .build() returns a Result, .unwrap() will panic if an error occurs.
    //    This assumes the input ethers::types::Transaction is always valid for TxEnv creation.
    builder.build().unwrap()
}
// pub fn simulate_v2_swap_safe(
//     amount_in: f64,
//     reserve_in: U256,
//     reserve_out: U256,
//     fee_bps: u32,         // fee in basis points (e.g., 25 for 0.25% PancakeSwap V2)
//     is_forward: bool,
// ) -> f64 {
//     // Add overflow protection for as_u128() calls
//     let r_in = if reserve_in > U256::from(u128::MAX) { 
//         u128::MAX as f64 
//     } else { 
//         reserve_in.as_u128() as f64 
//     };
//     let r_out = if reserve_out > U256::from(u128::MAX) { 
//         u128::MAX as f64 
//     } else { 
//         reserve_out.as_u128() as f64 
//     };
    
//     // PancakeSwap V2 uses 0.25% fee = 9975/10000
//     let fee_numerator = 10000.0 - fee_bps as f64;
//     let fee_denominator = 10000.0;

//     if r_in == 0.0 || r_out == 0.0 || amount_in == 0.0 {
//         return 0.0;
//     }

//     let amount_in_with_fee = amount_in * fee_numerator / fee_denominator;
//     let numerator = amount_in_with_fee * r_out;
//     let denominator = r_in * fee_denominator + amount_in_with_fee;
//     let output = numerator / denominator;

//     if output <= 0.0 || output.is_nan() || output.is_infinite() {
//         return 0.0;
//     }

//     output
// }

// /// Simulate V2 reverse swap (getAmountsIn) - calculates input needed for desired output
// /// Uses exact PancakeSwap V2 formula: amountIn = (reserveIn * amountOut * 10000) / ((reserveOut - amountOut) * 9975) + 1
// pub fn simulate_v2_swap_reverse_safe(
//     amount_out: f64,
//     reserve_in: U256,
//     reserve_out: U256,
//     fee_bps: u32,         // fee in basis points (e.g., 25 for 0.25% PancakeSwap V2)
//     is_forward: bool,
// ) -> f64 {
//     // Add overflow protection for as_u128() calls
//     let r_in = if reserve_in > U256::from(u128::MAX) { 
//         u128::MAX as f64 
//     } else { 
//         reserve_in.as_u128() as f64 
//     };
//     let r_out = if reserve_out > U256::from(u128::MAX) { 
//         u128::MAX as f64 
//     } else { 
//         reserve_out.as_u128() as f64 
//     };
    
//     // PancakeSwap V2 uses 0.25% fee = 9975/10000
//     let fee_numerator = 10000.0 - fee_bps as f64;
//     let fee_denominator = 10000.0;

//     if r_in == 0.0 || r_out == 0.0 || amount_out == 0.0 || amount_out >= r_out {
//         return 0.0;
//     }

//     let numerator = r_in * amount_out * fee_denominator;
//     let denominator = (r_out - amount_out) * fee_numerator;
//     let amount_in = numerator / denominator + 1.0; // +1 for rounding up

//     if amount_in <= 0.0 || amount_in.is_nan() || amount_in.is_infinite() {
//         return 0.0;
//     }

//     amount_in
// }

// /// Simulate V3 swap using proper Uniswap V3 math
// /// This function now uses the correct V3 formulas from v3_math.rs
// pub fn simulate_v3_swap_precise(
//     amount_in: f64,
//     sqrt_price_x96: U256,
//     liquidity: U256,
//     fee_bps: u32,
//     is_forward: bool,
// ) -> f64 {
//     use crate::v3_math::simulate_v3_swap;
    
//     // Convert is_forward to zero_for_one (V3 terminology)
//     // is_forward = true means token0->token1, which is zero_for_one = true
//     let zero_for_one = is_forward;
    
//     simulate_v3_swap(
//         U256::from(amount_in as u128),
//         sqrt_price_x96,
//         liquidity,
//         fee_bps,
//         zero_for_one,
//     ).map(|result| {
//         // Add overflow protection for as_u128() call
//         if result > U256::from(u128::MAX) {
//             u128::MAX as f64
//         } else {
//             result.as_u128() as f64
//         }
//     }).unwrap_or(0.0)
// }
