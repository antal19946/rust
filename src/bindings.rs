use ethers::contract::abigen;

// Uniswap V2 Pair ABI (getReserves)
abigen!(
    UniswapV2Pair,
    r#"[
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast)
        function token0() external view returns (address)
        function token1() external view returns (address)
    ]"#
);

// Uniswap V3 Pool ABI (slot0, liquidity, tickSpacing, fee)
abigen!(
    UniswapV3Pool,
    r#"[
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked)
        function liquidity() external view returns (uint128)
        function token0() external view returns (address)
        function token1() external view returns (address)
        function tickSpacing() external view returns (int24)
        function fee() external view returns (uint24)
        function factory() external view returns (address)
    ]"#
);

abigen!(
    DirectSwapExecutor,
    r#"[
        function executeSwap(address[],address[],uint8[],uint256[],bytes[],uint256)
        function buySellExecution(address[],address[],uint8[],uint256[],address[],address[],uint8[],uint256[])
        function withdrawToken(address,address,uint256)
    ]"#
);
