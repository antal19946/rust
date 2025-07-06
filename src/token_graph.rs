use ethers::types::H160;
use dashmap::DashMap;
use std::collections::HashMap;

use crate::cache::{ReserveCache, PoolType};
use crate::token_index::TokenIndexMap;

#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub to: u16,              // destination token index
    pub pool: H160,           // pool address
    pub pool_type: PoolType,  // V2 or V3
}

#[derive(Debug)]
pub struct TokenGraph {
    pub edges: DashMap<u16, Vec<GraphEdge>>, // token_index → list of outgoing edges
}

impl TokenGraph {
    pub fn build(
        reserve_cache: &ReserveCache,
        token_index: &TokenIndexMap,
    ) -> Self {
        let edges = DashMap::new();

        for entry in reserve_cache.iter() {
            let token0 = entry.value().token0;
            let token1 = entry.value().token1;
            let pool = *entry.key();
            let pool_type = entry.value().pool_type.clone();

            let index0 = token_index.address_to_index.get(&token0).unwrap();
            let index1 = token_index.address_to_index.get(&token1).unwrap();

            // Add edge: token0 → token1
            edges.entry(*index0).or_insert(Vec::new()).push(GraphEdge {
                to: *index1,
                pool,
                pool_type: pool_type.clone(),
            });

            // Add edge: token1 → token0
            edges.entry(*index1).or_insert(Vec::new()).push(GraphEdge {
                to: *index0,
                pool,
                pool_type: pool_type.clone(),
            });
        }

        Self { edges }
    }
} 