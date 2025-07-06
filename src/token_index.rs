use ethers::types::H160;
use std::collections::HashMap;
use crate::cache::ReserveCache;

#[derive(Debug)]
pub struct TokenIndexMap {
    pub address_to_index: HashMap<H160, u16>,
    pub index_to_address: HashMap<u16, H160>,
}

impl TokenIndexMap {
    pub fn build_from_reserve_cache(reserve_cache: &ReserveCache) -> Self {
        let mut address_to_index = HashMap::new();
        let mut index_to_address = HashMap::new();
        let mut next_index: u16 = 0;

        for entry in reserve_cache.iter() {
            let token0 = entry.value().token0;
            let token1 = entry.value().token1;

            for token in [token0, token1] {
                if !address_to_index.contains_key(&token) {
                    address_to_index.insert(token, next_index);
                    index_to_address.insert(next_index, token);
                    next_index += 1;
                }
            }
        }

        Self {
            address_to_index,
            index_to_address,
        }
    }
} 