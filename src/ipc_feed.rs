use crate::cache::ReserveCache;
use crate::config::Config;
use crate::mempool_decoder::ArbitrageOpportunity;
use crate::revm_sim::{RevmSimulator, print_dex_events_from_trace, print_full_call_trace};
use crate::route_cache::RoutePath;
use crate::token_index::TokenIndexMap;
use crate::token_tax::TokenTaxMap;
use crate::tx_decoder::Decoder;
use crate::utils::ethers_tx_to_revm_txenv;
use alloy_provider::DynProvider;
use dashmap::DashMap;
use ethers::providers::{Ipc, Middleware, Provider};
use ethers::types::Address;
use ethers::types::TxHash;
use ethers::types::{BlockId, BlockNumber};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

const SIM_WORKERS: usize = 32; // Number of parallel simulation workers (tune as needed)

// Helper: Load known routers from txt file (one address per line)
pub async fn load_known_routers(path: &str) -> anyhow::Result<HashSet<String>> {
    let mut set = HashSet::new();
    if let Ok(file) = File::open(path).await {
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            let addr = line.trim().to_lowercase();
            if !addr.is_empty() {
                set.insert(addr);
            }
        }
    }
    Ok(set)
}

// Helper: Append new router to txt file (if not already present)
pub async fn append_known_router(
    path: &str,
    addr: &str,
    cache: &Mutex<HashSet<String>>,
) -> anyhow::Result<()> {
    let mut cache = cache.lock().await;
    let addr = addr.to_lowercase();
    if !cache.contains(&addr) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(format!("{}\n", addr).as_bytes()).await?;
        cache.insert(addr);
    }
    Ok(())
}

pub async fn listen_and_fetch_details(
    _ws_url: &str,
    http_url: &str,
    dbprovider: Arc<DynProvider>,
    reserve_cache: &Arc<ReserveCache>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
) -> anyhow::Result<()> {
    // Use IPC for ultra-low-latency mempool listening
    let ipc_path = "/mnt/fillnode/bsc-node/geth.ipc";
    let ipc = Ipc::connect(ipc_path).await?;
    let provider = Provider::new(ipc);

    // Known routers cache setup
    let known_router_path = "data/known_routers.txt";
    let known_router_cache = Arc::new(Mutex::new(load_known_routers(known_router_path).await?));

    let (tx, mut rx) = mpsc::channel::<TxHash>(1024);
    let (sim_tx, sim_rx) = mpsc::channel::<(TxHash, ethers::types::Transaction, u64, u64)>(1024);
    let sim_rx = Arc::new(TokioMutex::new(sim_rx));

    let provider_listener = provider.clone();
    let tx_sender = tx.clone();
    tokio::spawn(async move {
        match provider_listener.subscribe_pending_txs().await {
            Ok(mut stream) => {
                while let Some(tx_hash) = stream.next().await {
                    if tx_sender.send(tx_hash).await.is_err() {
                        break;
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to subscribe to pending txs: {:?}", e);
            }
        }
    });

    // Spawn simulation workers
    for _ in 0..SIM_WORKERS {
        let reserve_cache = reserve_cache.clone();
        let token_index = token_index.clone();
        let precomputed_route_cache = precomputed_route_cache.clone();
        let token_tax_map = token_tax_map.clone();
        let config = config.clone();
        let opportunity_tx = opportunity_tx.clone();
        let sim_rx = sim_rx.clone();
        let revm_sim = RevmSimulator::new();
        let http_url = http_url.to_string();
        let dbProvider = dbprovider.clone();
        tokio::spawn(async move {
            loop {
                let next = {
                    let mut locked = sim_rx.lock().await;
                    locked.recv().await
                };
                if let Some((tx_hash, tx, sim_block, sim_block_ts)) = next {
                    let sim_start = Instant::now();
                    let to_addr = tx.to.map(|a| format!("0x{:x}", a));
                    let tx_env = crate::utils::ethers_tx_to_revm_txenv(&tx);
                    let tx_hash_hex = hex::encode(tx.hash);
                    println!(
                        "[DEBUG] Simulation start: tx {:?}, block {}, ts {}",
                        tx_hash, sim_block, sim_block_ts
                    );
                    let sim_latency_revmstart = sim_start.elapsed().as_millis();
                    println!(
                        "[DEBUG] Simulation revm  start latency for tx {:?}: {} ms",
                        tx_hash, sim_latency_revmstart
                    );
                    let trace_opt = revm_sim
                        .simulate_with_forked_state(tx_env.clone(), dbProvider.clone())
                        .await
                        .unwrap_or(None);
                    let sim_latency_revm = sim_start.elapsed().as_millis();
                    println!(
                        "[DEBUG] Simulation revm latency for tx {:?}: {} ms",
                        tx_hash, sim_latency_revm
                    );
                    if let Some(trace) = trace_opt {
                        print_dex_events_from_trace(
                            &trace,
                            &tx_hash_hex,
                            &reserve_cache,
                            &token_index,
                            &precomputed_route_cache,
                            &token_tax_map,
                            &config,
                            &opportunity_tx,
                        ).await;
                    } else {
                        println!("No call trace produced for tx: {}", tx_hash_hex);
                    }
                    let sim_latency = sim_start.elapsed().as_millis();
                    println!(
                        "[DEBUG] Simulation latency for tx {:?}: {} ms",
                        tx_hash, sim_latency
                    );
                } else {
                    break;
                }
            }
        });
    }

    while let Some(tx_hash) = rx.recv().await {
        let provider = provider.clone();
        let known_router_cache = known_router_cache.clone();
        let sim_tx = sim_tx.clone();
        tokio::spawn(async move {
            let sim_block = provider.get_block_number().await.unwrap_or_default();
            let sim_block_ts = provider
                .get_block(BlockId::Number(BlockNumber::Number(sim_block)))
                .await
                .ok()
                .flatten()
                .map(|b| b.timestamp.as_u64())
                .unwrap_or(0);
            if let Ok(tx) = provider.get_transaction(tx_hash).await {
                if let Some(tx) = tx {
                    let to_addr = tx.to.map(|a| format!("0x{:x}", a));
                    let is_known = if let Some(addr) = &to_addr {
                        let cache = known_router_cache.lock().await;
                        cache.contains(addr)
                    } else {
                        false
                    };
                    if is_known {
                        // Send to simulation worker queue
                        let _ = sim_tx
                            .send((tx_hash, tx, sim_block.as_u64(), sim_block_ts))
                            .await;
                    }
                }
            }
        });
    }
    Ok(())
}
