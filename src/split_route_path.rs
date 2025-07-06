use crate::route_cache::RoutePath;

#[inline]
pub fn split_route_around_token_x(
    route: &RoutePath,
    token_x_idx: u32,
) -> Option<(RoutePath, RoutePath)> {
    let token_pos = route.hops.iter().position(|&t| t == token_x_idx)?;

    // Define buy and sell hops
    let buy_hops = route.hops[0..=token_pos].to_vec();     // includes tokenX
    let sell_hops = route.hops[token_pos..].to_vec();      // starts from tokenX

    // Corresponding pool slices
    let buy_pool_len = buy_hops.len().saturating_sub(1);
    let sell_pool_len = sell_hops.len().saturating_sub(1);

    let buy_path = RoutePath {
        hops: buy_hops,
        pools: route.pools[0..buy_pool_len].to_vec(),
        dex_types: route.dex_types[0..buy_pool_len].to_vec(),
    };

    let sell_path = RoutePath {
        hops: sell_hops,
        pools: route.pools[route.pools.len() - sell_pool_len..].to_vec(),
        dex_types: route.dex_types[route.dex_types.len() - sell_pool_len..].to_vec(),
    };

    Some((buy_path, sell_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::H160;
    use crate::route_cache::{RoutePath, DEXType};

    #[test]
    fn test_split() {
        let route = RoutePath {
            hops: vec![1, 2, 3, 4],
            pools: vec![H160::zero(), H160::zero(), H160::zero()],
            dex_types: vec![DEXType::PancakeV2, DEXType::BiSwapV2, DEXType::ApeSwapV2],
        };

        let (buy, sell) = split_route_around_token_x(&route, 3).unwrap();
        assert_eq!(buy.hops, vec![1, 2, 3]);
        assert_eq!(sell.hops, vec![3, 4]);
    }
} 