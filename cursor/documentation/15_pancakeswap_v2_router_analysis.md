# PancakeSwap V2 Router Analysis & Implementation

## Research Summary

Based on deep analysis of PancakeSwap V2 Router contract, the following key findings were implemented:

### 1. Fee Structure
- **PancakeSwap V2 uses 0.25% fee per swap**
- **Fee multiplier: 9975/10000** (not 997/1000 like Uniswap V2)
- This is encoded in the formulas as `9975` and `10000` constants

### 2. Core Formulas

#### getAmountsOut (Sell Simulation)
```
amountOut = (amountIn * 9975 * reserveOut) / (reserveIn * 10000 + amountIn * 9975)
```

#### getAmountsIn (Buy Simulation)  
```
amountIn = (reserveIn * amountOut * 10000) / ((reserveOut - amountOut) * 9975) + 1
```

### 3. Implementation Details

#### V2 Sell Path (Forward)
- Uses `getAmountsOut` formula
- Applies 0.25% fee deduction
- Iterates through pools in forward direction

#### V2 Buy Path (Reverse)
- Uses `getAmountsIn` formula  
- Calculates input needed for desired output
- Iterates through pools in reverse direction
- Adds +1 for rounding up

### 4. Key Differences from Uniswap V2

| Aspect | Uniswap V2 | PancakeSwap V2 |
|--------|------------|----------------|
| Fee | 0.3% (997/1000) | 0.25% (9975/10000) |
| Factory | 0x5C69bEe... | 0xcA143Ce... |
| Router | 0x7a250d... | 0x10ED43C... |

### 5. Code Implementation

#### simulate_swap_path.rs
- ✅ Correct 9975/10000 fee implementation
- ✅ Proper getAmountsIn/getAmountsOut formulas
- ✅ Debug prints for troubleshooting
- ✅ Error handling for edge cases

#### utils.rs  
- ✅ Updated `simulate_v2_swap_safe()` with correct fee
- ✅ Added `simulate_v2_swap_reverse_safe()` for buy simulation
- ✅ PancakeSwap V2 specific constants

### 6. Testing & Verification

#### Test Function
```rust
pub fn test_pancakeswap_v2_simulation()
```
- Verifies formulas match PancakeSwap Router behavior
- Tests both buy and sell scenarios
- Uses realistic reserve values

#### Debug Output
- Detailed logging for V2 BUY/SELL operations
- Pool state information
- Reserve and amount calculations

### 7. Integration Points

#### Route Cache
- Precomputed paths work with V2 simulation
- Fee-aware route selection
- Pool type detection (V2 vs V3)

#### Reserve Cache  
- Real-time reserve updates via Sync events
- Fee information per pool
- Token0/token1 ordering

### 8. Performance Optimizations

#### Memory Efficiency
- U256 for precise calculations
- No floating point errors
- Efficient reserve caching

#### Speed
- Direct formula application
- No external calls
- Parallel path simulation

### 9. Error Handling

#### Edge Cases
- Zero reserves detection
- Insufficient liquidity
- Division by zero prevention
- Overflow protection

#### Debug Information
- Pool addresses in logs
- Reserve values
- Calculation steps
- Failure reasons

### 10. Future Improvements

#### Potential Enhancements
- Support for different DEX fees (BiSwap 0.1%, ApeSwap 0.2%)
- Dynamic fee detection from pool contracts
- Slippage tolerance integration
- Gas optimization for multi-hop paths

## Conclusion

The implementation now accurately matches PancakeSwap V2 Router behavior:
- ✅ Correct fee calculation (0.25%)
- ✅ Exact formula implementation
- ✅ Proper buy/sell simulation
- ✅ Comprehensive error handling
- ✅ Debug and testing capabilities

This ensures arbitrage calculations are accurate and profitable. 