# 🚀 Ultra-Low Latency Arbitrage Bot - Complete Function Documentation

A high-performance arbitrage bot for Binance Smart Chain (BSC) built in Rust with ultra-low latency optimization, real-time mempool monitoring, and sniper-grade arbitrage detection.

## 📁 Project Structure & Function Documentation

### 🔧 Core Configuration (`src/config.rs`)

#### Structs
- **`Config`**: Main configuration struct containing all bot settings
- **`DexConfig`**: DEX-specific configuration (factory address, fee, etc.)
- **`BaseToken`**: Base token configuration (address, symbol, decimals)

#### Functions
- **`Config::default()`**: Creates default configuration with BSC DEXes
- **`Config::get_dex_by_name(name: &str)`**: Returns DEX config by name
- **`Config::get_base_token_by_symbol(symbol: &str)`**: Returns base token by symbol
- **`Config::get_base_token_by_address(address: Address)`**: Returns base token by address
- **`Config::get_v2_dexes()`**: Returns list of V2 DEX configurations
- **`Config::get_v3_dexes()`**: Returns list of V3 DEX configurations
- **`Config::get_stable_tokens()`**: Returns list of stable token configurations

### 🗄️ Cache Management (`src/cache.rs`)

#### Structs
- **`PoolState`**: Individual pool state (reserves, sqrtPrice, liquidity, etc.)
- **`ReserveCache`**: Thread-safe cache for pool states using DashMap
- **`TokenMeta`**: Token metadata (safety status, transfer tax info)

#### Functions
- **`ReserveCache::default()`**: Creates new empty reserve cache
- **`ReserveCache::get(pool_address)`**: Gets pool state by address
- **`ReserveCache::insert(pool_address, state)`**: Inserts/updates pool state
- **`ReserveCache::len()`**: Returns number of cached pools
- **`preload_reserve_cache(pairs, provider, cache, batch_size)`**: Preloads all pool reserves in parallel

### 🔗 Smart Contract Bindings (`src/bindings.rs`)

#### Generated Bindings
- **`UniswapV2Pair`**: PancakeSwap V2 pair contract bindings
- **`UniswapV3Pool`**: PancakeSwap V3 pool contract bindings
- **`IUniswapV2Factory`**: Factory contract bindings for pair creation events

### 📊 Token Indexing (`src/token_index.rs`)

#### Structs
- **`TokenIndexMap`**: Bidirectional mapping between token addresses and indices

#### Functions
- **`TokenIndexMap::build_from_reserve_cache(cache)`**: Builds token index from reserve cache
- **`TokenIndexMap::get_index(address)`**: Gets token index by address
- **`TokenIndexMap::get_address(index)`**: Gets token address by index

### 🕸️ Token Graph (`src/token_graph.rs`)

#### Structs
- **`TokenGraph`**: Graph representation of token relationships
- **`GraphEdge`**: Edge in token graph (pool info, reserves, etc.)

#### Functions
- **`TokenGraph::build(reserve_cache, token_index)`**: Builds token graph from cache
- **`TokenGraph::get_edges(token_index)`**: Gets all edges for a token

### 🛣️ Route Cache (`src/route_cache.rs`)

#### Structs
- **`RoutePath`**: Precomputed route with hops, pools, and DEX types
- **`PoolMeta`**: Pool metadata for route building
- **`DEXType`**: Enum for different DEX types

#### Functions
- **`build_route_cache(all_tokens, all_pools, base_tokens)`**: Builds precomputed route cache
- **`find_2hop_routes(base_tokens, all_tokens, pool_lookup)`**: Finds 2-hop arbitrage routes
- **`find_3hop_routes(base_tokens, all_tokens, pool_lookup)`**: Finds 3-hop arbitrage routes

### 🔍 Best Route Finder (`src/best_route_finder.rs`)

#### Structs
- **`RoutePath`**: Route with hops, pools, DEX types, and output
- **`BestRoute`**: Best buy and sell routes for a token
- **`PartialRoute`**: Partial route during DFS search

#### Functions
- **`dfs_all_paths(current, target, depth, graph, visited)`**: DFS to find all paths between tokens
- **`simulate_path(route, reserve_cache, token_index)`**: Simulates a route to get output amount
- **`generate_best_routes_for_token(token_x, base_tokens, graph, reserve_cache, token_index)`**: Finds best routes for a token
- **`populate_best_routes_for_all_tokens(graph, reserve_cache, token_index, base_tokens, tracked_tokens, route_cache)`**: Populates best routes for all tokens in parallel

### 🔄 Split Route Path (`src/split_route_path.rs`)

#### Functions
- **`split_route_around_token_x(route, token_x_index)`**: Splits a route into buy and sell paths around a token

### 🧮 V3 Math (`src/v3_math.rs`)

#### Constants
- **`Q96`**: 2^96 for sqrtPriceX96 calculations

#### Functions
- **`mul_div(a, b, denominator)`**: Safe multiplication and division
- **`sqrt_price_x96_to_price(sqrt_price_x96)`**: Converts sqrtPriceX96 to price
- **`price_to_sqrt_price_x96(price)`**: Converts price to sqrtPriceX96
- **`simulate_v3_swap(amount_in, sqrt_price_x96, liquidity, fee, zero_for_one)`**: Simulates V3 swap (exact input)
- **`calculate_v3_buy_amount(amount_out, sqrt_price_x96, liquidity, fee, zero_for_one)`**: Calculates input needed for exact output
- **`get_next_sqrt_price_from_input(sqrt_price_x96, liquidity, amount_in, zero_for_one)`**: Gets next sqrtPrice after input
- **`get_next_sqrt_price_from_output(sqrt_price_x96, liquidity, amount_out, zero_for_one)`**: Gets next sqrtPrice after output
- **`test_v3_math()`**: Comprehensive V3 math tests

### 🎯 Simulate Swap Path (`src/simulate_swap_path.rs`)

#### Structs
- **`HopDetail`**: Detailed information about each hop in a path
- **`PathSimulationResult`**: Result of path simulation with amounts and hops
- **`RouteSimulationResult`**: Result of route simulation with buy/sell paths
- **`ComprehensiveSimulationResults`**: Comprehensive results for all routes

#### Functions
- **`simulate_v3_swap_single(amount_in, sqrt_price_x96, liquidity, fee, zero_for_one)`**: Single V3 swap simulation
- **`simulate_buy_path(route, token_x_amount, cache, token_index_map)`**: Simulates buy path (base → tokenX)
- **`simulate_sell_path(route, token_x_amount, cache, token_index_map)`**: Simulates sell path (tokenX → base)
- **`simulate_buy_path_amounts_vec(route, token_x_amount, cache, token_index_map)`**: Returns (amounts_in, amounts_out) for buy path
- **`simulate_sell_path_amounts_vec(route, token_x_amount, cache, token_index_map)`**: Returns (amounts_in, amounts_out) for sell path
- **`simulate_buy_path_amounts_array(route, token_x_amount, cache, token_index_map)`**: Returns Router format array for buy path
- **`simulate_sell_path_amounts_array(route, token_x_amount, cache, token_index_map)`**: Returns Router format array for sell path
- **`simulate_all_filtered_routes(token_address, pool_address, token_x_amount, all_tokens, precomputed_route_cache, reserve_cache, token_index_map)`**: Simulates all routes containing a specific pool
- **`print_comprehensive_results(results)`**: Prints comprehensive simulation results
- **`test_pancakeswap_v2_simulation()`**: Tests V2 simulation accuracy
- **`test_v3_simulation()`**: Tests V3 simulation accuracy
- **`print_path_simulation_details(result, path_name)`**: Prints detailed path simulation info

### 🎯 Arbitrage Finder (`src/arbitrage_finder.rs`)

#### Structs
- **`SimulatedRoute`**: Complete arbitrage route with merged amounts and tokens

#### Functions
- **`simulate_all_paths_for_token_x(token_x_index, token_x_amount, pool_address, precomputed_route_cache, reserve_cache, token_index_map)`**: Finds all arbitrage paths for a token
- **`print_simulated_route(route)`**: Prints detailed arbitrage route information

### 📡 Mempool Decoder (`src/mempool_decoder.rs`)

#### Structs
- **`DecodedSwap`**: Decoded swap information from mempool
- **`ArbitrageOpportunity`**: Complete arbitrage opportunity with routes
- **`MempoolDecoder`**: Main mempool decoder struct
- **`SwapInfo`**: Swap information for processing

#### Functions
- **`start_mempool_monitoring(ws_provider, reserve_cache, token_index, precomputed_route_cache, config)`**: Starts mempool monitoring
- **`MempoolDecoder::new(config)`**: Creates new mempool decoder
- **`MempoolDecoder::start_monitoring()`**: Starts monitoring mempool transactions
- **`MempoolDecoder::process_v2_swap_event(log, reserve_cache, token_index, precomputed_route_cache, opportunity_tx)`**: Processes V2 swap events
- **`MempoolDecoder::process_v3_swap_event(log, reserve_cache, token_index, precomputed_route_cache, opportunity_tx)`**: Processes V3 swap events
- **`MempoolDecoder::find_arbitrage_opportunities(token_x, token_x_amount, pool_address, reserve_cache, token_index, precomputed_route_cache)`**: Finds arbitrage opportunities for a token
- **`MempoolDecoder::log_opportunity(opportunity)`**: Logs arbitrage opportunity to file
- **`MempoolDecoder::get_hourly_profit_summary()`**: Gets hourly profit summary

### 📊 Price Tracker (`src/price_tracker.rs`)

#### Functions
- **`start_price_tracker(ws_provider, provider, reserve_cache, token_index, precomputed_route_cache, opportunity_tx)`**: Starts price tracker
- **`handle_v2_sync_event(log, reserve_cache, token_index, precomputed_route_cache, opportunity_tx)`**: Handles V2 Sync events
- **`handle_v3_swap_event(log, reserve_cache, token_index, precomputed_route_cache, opportunity_tx)`**: Handles V3 Swap events
- **`find_arbitrage_opportunities(token_x, token_x_amount, pool_address, reserve_cache, token_index, precomputed_route_cache, opportunity_tx)`**: Finds arbitrage opportunities from price events

### 🛠️ Utils (`src/utils.rs`)

#### Functions
- **`simulate_v2_swap_safe(amount_in, reserve_in, reserve_out, fee, is_forward)`**: Safe V2 swap simulation
- **`simulate_v3_swap_precise(amount_in, sqrt_price_x96, liquidity, fee, zero_for_one)`**: Precise V3 swap simulation
- **`simulate_v2_swap_reverse_safe(amount_out, reserve_in, reserve_out, fee, is_forward)`**: Safe reverse V2 swap simulation

### 🔄 Fetch Pairs (`src/fetch_pairs.rs`)

#### Structs
- **`PairInfo`**: Pair information from factory events
- **`PairFetcher`**: Main pair fetcher struct
- **`FactoryProgress`**: Progress tracking for factory fetching

#### Functions
- **`PairFetcher::new(config)`**: Creates new pair fetcher
- **`PairFetcher::load_progress()`**: Loads factory progress from file
- **`PairFetcher::save_progress()`**: Saves factory progress to file
- **`PairFetcher::fetch_all_pairs()`**: Fetches all pairs from all factories
- **`PairFetcher::fetch_factory_pairs(factory_address, dex_name)`**: Fetches pairs from specific factory
- **`PairFetcher::parse_pair_created_log(log, dex)`**: Parses PairCreated event
- **`PairFetcher::parse_pool_created_log(log, dex)`**: Parses PoolCreated event
- **`load_safe_tokens(path)`**: Loads safe tokens from JSON file

### 🚀 Main Application (`src/main.rs`)

#### Main Functions
- **`main()`**: Main application entry point
  - Loads configuration
  - Tests V3 math functions
  - Loads pairs from files
  - Builds providers and cache
  - Preloads reserves
  - Builds token index and graph
  - Populates best routes
  - Starts mempool monitoring
  - Starts price tracker
  - Processes arbitrage opportunities

## 🚀 Quick Start

### Prerequisites
- Rust 1.70+
- Linux (recommended for performance)
- 8GB+ RAM
- Fast internet connection for BSC RPC

### Installation

1. **Clone and build**
```bash
git clone <repository>
cd arb-rust-bot
cargo build --release
```

2. **Configure environment**
```bash
cp env.example .env
# Edit .env with your RPC endpoints and wallet
```

3. **Run the bot**
```bash
cargo run --release
```

## 📊 Performance Features

- **Real-time mempool monitoring**: Sub-second arbitrage detection
- **Parallel processing**: Rayon-based parallel route simulation
- **Memory optimization**: Efficient data structures and caching
- **V2/V3 support**: Complete support for both Uniswap V2 and V3 style pools
- **Multi-DEX**: PancakeSwap, BiSwap, ApeSwap, and more
- **Safety checks**: Honeypot detection and safe token validation

## 🔧 Configuration

### Environment Variables
```bash
# RPC Endpoints
HTTP_RPC=https://bsc-dataseed1.binance.org/
WS_RPC=wss://bsc-ws-node.nariox.org:443

# Base Tokens (USDT, USDC, WBNB, etc.)
USDT_ADDRESS=0x55d398326f99059fF775485246999027B3197955
USDC_ADDRESS=0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d
WBNB_ADDRESS=0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c
```

## 📈 Usage Examples

### Basic Usage
```bash
# Start the bot
cargo run --release

# Monitor logs
tail -f run.log
```

### Advanced Usage
```bash
# Run with debug logging
RUST_LOG=debug cargo run --release

# Profile performance
perf record -g cargo run --release
```

## 🛡️ Safety Features

- **Honeypot detection**: Validates tokens before trading
- **Transfer tax analysis**: Checks for high transfer taxes
- **Liquidity validation**: Ensures sufficient liquidity
- **Slippage protection**: Calculates realistic slippage
- **Profit thresholds**: Minimum profit requirements

## 📚 Technical Details

### Arbitrage Detection Flow
1. **Event Detection**: Mempool decoder and price tracker detect swaps
2. **Token Identification**: Identifies affected token and amount
3. **Route Filtering**: Filters precomputed routes containing affected pool
4. **Path Simulation**: Simulates buy and sell paths for each route
5. **Profit Calculation**: Calculates profit/loss for each route
6. **Opportunity Selection**: Selects most profitable route
7. **Execution**: Ready for arbitrage execution

### V2 vs V3 Math
- **V2**: Constant product formula with closed-form solutions
- **V3**: Concentrated liquidity with binary search for optimal amounts

### Memory Management
- **DashMap**: Thread-safe concurrent hash maps
- **Arc**: Atomic reference counting for shared data
- **Efficient structs**: Optimized data structures for performance

## 🔍 Troubleshooting

### Common Issues
1. **RPC connection issues**: Check network and RPC endpoint
2. **Memory usage**: Monitor RAM usage, reduce cache size if needed
3. **No profitable routes**: Check profit thresholds and market conditions
4. **V3 math errors**: Ensure proper sqrtPriceX96 and liquidity values

### Performance Optimization
1. **Use local BSC node**: Reduces latency significantly
2. **Increase profit threshold**: Reduces false positives
3. **Optimize route cache**: Precompute more routes for faster detection
4. **CPU pinning**: Pin critical threads to specific CPU cores

## 📄 License

This project is for educational and research purposes. Use at your own risk.

## 🤝 Contributing

Contributions are welcome! Please read the code documentation and test thoroughly before submitting changes.
