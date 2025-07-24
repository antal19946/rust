//! REVM Simulation Manager: Custom EVM/Handler pattern for stateless simulation & tracing

use anyhow::Result;
use chrono::{DateTime, Utc};
use revm::context_interface::result::Output;
use revm::database::{CacheDB, EmptyDB};
use revm::{
    Context, MainContext,
    context::{BlockEnv, CfgEnv, ContextSetters, TxEnv},
    database::InMemoryDB,
    handler::{
        EvmTr, ExecuteEvm, FrameResult, Handler, PrecompileProvider, evm::FrameTr,
        instructions::InstructionProvider,
    },
    inspector::{Inspector, InspectorEvmTr, InspectorHandler},
    interpreter::{InterpreterResult, interpreter::EthInterpreter, interpreter_action::FrameInit},
    primitives::{Address as RevmAddress, Bytes as RevmBytes},
};
use serde::Serialize;
use std::fs::OpenOptions;
use std::{marker::PhantomData, time::Instant};
// use revm::inspector::InspectorHandler;
use crate::mempool_decoder::{ArbitrageOpportunity, DecodedSwap};
use crate::route_cache::RoutePath;
use crate::token_index::TokenIndexMap;
use crate::token_tax::TokenTaxMap;
use crate::{
    cache::ReserveCache,
    config::Config,
    simulate_swap_path::{simulate_buy_path_amounts_array, simulate_sell_path_amounts_array},
    split_route_path::split_route_around_token_x,
};
use alloy_eips::BlockId;
use alloy_primitives::keccak256;
use alloy_provider::{DynProvider, Provider, ProviderBuilder, network::Ethereum};
use dashmap::DashMap;
use ethers::abi::{ParamType, Token};
use ethers::types::Transaction;
use ethers::types::{Bytes as eBytes, H160, U256 as eU256};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use revm::bytecode::Bytecode;
use revm::database::{AlloyDB, WrapDatabaseAsync};
use revm::primitives::B256;
use revm::state::AccountInfo;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
// use revm::database_interface::WrapDatabaseAsync,
// --- Simulation Result Types ---
#[derive(Debug, Clone, Serialize)]
pub struct SimLog {
    pub address: Vec<u8>,
    pub topics: Vec<Vec<u8>>, // TODO: topics extraction not possible due to private field
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimResult {
    pub status: String,
    pub gas_used: u64,
    pub output: Option<Vec<u8>>,
    pub logs: Vec<SimLog>,
}
pub fn parse_logdata_string2(logdata_bytes: &[u8]) -> (Vec<String>, String) {
    let logdata = String::from_utf8_lossy(logdata_bytes);

    // Find the "LogData {" substring
    let logdata_start = match logdata.find("LogData {") {
        Some(idx) => idx,
        None => return (vec![], String::new()),
    };

    // Find the closing '}' for LogData { ... }
    let mut brace_count = 0;
    let mut end_idx = None;
    for (i, c) in logdata[logdata_start..].char_indices() {
        if c == '{' {
            brace_count += 1;
        } else if c == '}' {
            brace_count -= 1;
            if brace_count == 0 {
                end_idx = Some(logdata_start + i + 1);
                break;
            }
        }
    }
    let logdata_sub = match end_idx {
        Some(end) => &logdata[logdata_start..end],
        None => &logdata[logdata_start..],
    };

    // Now parse topics and data as before, but only in logdata_sub
    let topics_start = match logdata_sub.find("topics: [") {
        Some(idx) => idx + 9,
        None => return (vec![], String::new()),
    };
    let topics_end = match logdata_sub[topics_start..].find("]") {
        Some(rel_idx) => topics_start + rel_idx,
        None => return (vec![], String::new()),
    };
    let topics_str = &logdata_sub[topics_start..topics_end];
    let topics: Vec<String> = topics_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with("0x"))
        .collect();

    let data_start = match logdata_sub.find("data: ") {
        Some(idx) => idx + 6,
        None => return (topics, String::new()),
    };
    let data_end = logdata_sub[data_start..]
        .find('}')
        .map(|i| data_start + i)
        .unwrap_or(logdata_sub.len());
    let data_hex = logdata_sub[data_start..data_end].trim().to_string();

    (topics, data_hex)
}
/// Helper to parse stringified LogData from SimLog.data and extract topics/data as hex strings.
pub fn parse_logdata_string(logdata_bytes: &[u8]) -> (Vec<String>, String) {
    let logdata = String::from_utf8_lossy(logdata_bytes);
    // Extract topics
    let topics_start = match logdata.find("topics: [") {
        Some(idx) => idx + 9,
        None => return (vec![], String::new()),
    };
    let topics_end = match logdata[topics_start..].find("]") {
        Some(rel_idx) => topics_start + rel_idx,
        None => return (vec![], String::new()),
    };
    let topics_str = &logdata[topics_start..topics_end];
    let topics: Vec<String> = topics_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with("0x"))
        .collect();
    // Extract data
    let data_start = match logdata.find("data: ") {
        Some(idx) => idx + 6,
        None => return (topics, String::new()),
    };
    // Data ends at '}' or end of string
    let data_end = logdata[data_start..]
        .find('}')
        .map(|i| data_start + i)
        .unwrap_or(logdata.len());
    let data_field = logdata[data_start..data_end].trim();
    // Only take the first 0x... word (ignore trailing text)
    let data_hex = data_field
        .split_whitespace()
        .find(|s| s.starts_with("0x"))
        .unwrap_or("")
        .to_string();
    (topics, data_hex)
}

/// Pretty-print all logs in a SimResult, extracting topics/data from stringified LogData.
pub fn print_simresult_logs(sim_result: &SimResult) {
    for (i, sim_log) in sim_result.logs.iter().enumerate() {
        let (topics, data_hex) = parse_logdata_string(&sim_log.data);
        println!("Log #{}", i);
        println!("  Address: 0x{}", hex::encode(&sim_log.address));
        for (j, topic) in topics.iter().enumerate() {
            println!("    topics[{}]: {}", j, topic);
        }
        println!("    data: {}", data_hex);
    }
}

// --- MyEvm wrapper (REVM example style) ---
use revm::{
    context::{ContextTr, Evm, FrameStack},
    handler::{EthFrame, EthPrecompiles, instructions::EthInstructions},
};

#[derive(Debug)]
pub struct MyEvm<CTX, INSP>(
    pub  Evm<
        CTX,
        INSP,
        EthInstructions<EthInterpreter, CTX>,
        EthPrecompiles,
        EthFrame<EthInterpreter>,
    >,
);

impl<CTX: ContextTr, INSP> MyEvm<CTX, INSP> {
    pub fn new(ctx: CTX, inspector: INSP) -> Self {
        Self(Evm {
            ctx,
            inspector,
            instruction: EthInstructions::new_mainnet(),
            precompiles: EthPrecompiles::default(),
            frame_stack: FrameStack::new(),
        })
    }
}

impl<CTX: ContextTr, INSP> revm::handler::EvmTr for MyEvm<CTX, INSP> {
    type Context = CTX;
    type Instructions = EthInstructions<EthInterpreter, CTX>;
    type Precompiles = EthPrecompiles;
    type Frame = EthFrame<EthInterpreter>;
    fn ctx(&mut self) -> &mut Self::Context {
        &mut self.0.ctx
    }
    fn ctx_ref(&self) -> &Self::Context {
        self.0.ctx_ref()
    }
    fn ctx_instructions(&mut self) -> (&mut Self::Context, &mut Self::Instructions) {
        self.0.ctx_instructions()
    }
    fn ctx_precompiles(&mut self) -> (&mut Self::Context, &mut Self::Precompiles) {
        self.0.ctx_precompiles()
    }
    fn frame_stack(&mut self) -> &mut FrameStack<Self::Frame> {
        self.0.frame_stack()
    }
    fn frame_init(
        &mut self,
        frame_input: <Self::Frame as revm::handler::evm::FrameTr>::FrameInit,
    ) -> Result<
        revm::handler::ItemOrResult<
            &mut Self::Frame,
            <Self::Frame as revm::handler::evm::FrameTr>::FrameResult,
        >,
        revm::context::ContextError<
            <<Self::Context as ContextTr>::Db as revm::context_interface::Database>::Error,
        >,
    > {
        self.0.frame_init(frame_input)
    }
    fn frame_run(
        &mut self,
    ) -> Result<
        revm::handler::FrameInitOrResult<Self::Frame>,
        revm::context::ContextError<
            <<Self::Context as ContextTr>::Db as revm::context_interface::Database>::Error,
        >,
    > {
        self.0.frame_run()
    }
    fn frame_return_result(
        &mut self,
        frame_result: <Self::Frame as revm::handler::evm::FrameTr>::FrameResult,
    ) -> Result<
        Option<<Self::Frame as revm::handler::evm::FrameTr>::FrameResult>,
        revm::context::ContextError<
            <<Self::Context as ContextTr>::Db as revm::context_interface::Database>::Error,
        >,
    > {
        self.0.frame_return_result(frame_result)
    }
}

impl<CTX: ContextTr, INSP> revm::inspector::InspectorEvmTr for MyEvm<CTX, INSP>
where
    CTX: ContextSetters<Journal: revm::inspector::JournalExt>,
    INSP: revm::inspector::Inspector<CTX, EthInterpreter>,
{
    type Inspector = INSP;
    fn inspector(&mut self) -> &mut Self::Inspector {
        self.0.inspector()
    }
    fn ctx_inspector(&mut self) -> (&mut Self::Context, &mut Self::Inspector) {
        self.0.ctx_inspector()
    }
    fn ctx_inspector_frame(
        &mut self,
    ) -> (&mut Self::Context, &mut Self::Inspector, &mut Self::Frame) {
        self.0.ctx_inspector_frame()
    }
    fn ctx_inspector_frame_instructions(
        &mut self,
    ) -> (
        &mut Self::Context,
        &mut Self::Inspector,
        &mut Self::Frame,
        &mut Self::Instructions,
    ) {
        self.0.ctx_inspector_frame_instructions()
    }
}

// --- Handler and InspectorHandler for MyHandler<MyEvm<CTX, INSP>> ---
#[derive(Debug)]
pub struct MyHandler<EVM> {
    pub _phantom: PhantomData<EVM>,
}

impl<EVM> Default for MyHandler<EVM> {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<EVM> Handler for MyHandler<EVM>
where
    EVM: revm::handler::EvmTr<
            Context: revm::context_interface::ContextTr<
                Journal: revm::context_interface::JournalTr<State = revm::state::EvmState>,
            >,
            Precompiles: PrecompileProvider<EVM::Context, Output = InterpreterResult>,
            Instructions: InstructionProvider<
                Context = EVM::Context,
                InterpreterTypes = EthInterpreter,
            >,
            Frame: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
        >,
{
    type Evm = EVM;
    type Error = revm::context_interface::result::EVMError<<<EVM::Context as revm::context_interface::ContextTr>::Db as revm::context_interface::Database>::Error, revm::context::result::InvalidTransaction>;
    type HaltReason = revm::context::result::HaltReason;
    fn reward_beneficiary(
        &self,
        _evm: &mut Self::Evm,
        _exec_result: &mut FrameResult,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<EVM> InspectorHandler for MyHandler<EVM>
where
    EVM: InspectorEvmTr<
            Inspector: Inspector<<<Self as Handler>::Evm as EvmTr>::Context, EthInterpreter>,
            Context: ContextTr<Journal: JournalTr<State = EvmState>>,
            Precompiles: PrecompileProvider<EVM::Context, Output = InterpreterResult>,
            Instructions: InstructionProvider<
                Context = EVM::Context,
                InterpreterTypes = EthInterpreter,
            >,
        >,
{
    type IT = EthInterpreter;
}

// --- ExecuteEvm implementation for MyEvm ---
use revm::{
    context::result::ExecResultAndState,
    context_interface::{
        Database, JournalTr,
        result::{EVMError, ExecutionResult},
    },
    state::EvmState,
};

type MyError<CTX> = EVMError<
    <<CTX as ContextTr>::Db as Database>::Error,
    revm::context::result::InvalidTransaction,
>;

impl<CTX, INSP> ExecuteEvm for MyEvm<CTX, INSP>
where
    CTX: ContextSetters<Journal: JournalTr<State = EvmState>>,
{
    type State = EvmState;
    type ExecutionResult = ExecutionResult<revm::context::result::HaltReason>;
    type Error = MyError<CTX>;

    type Tx = <CTX as ContextTr>::Tx;
    type Block = <CTX as ContextTr>::Block;

    fn set_block(&mut self, block: Self::Block) {
        self.0.ctx.set_block(block);
    }

    fn transact_one(&mut self, tx: Self::Tx) -> Result<Self::ExecutionResult, Self::Error> {
        self.0.ctx.set_tx(tx);
        let mut handler = MyHandler::default();
        handler.run(self)
    }

    fn finalize(&mut self) -> Self::State {
        self.ctx().journal_mut().finalize()
    }

    fn replay(
        &mut self,
    ) -> Result<ExecResultAndState<Self::ExecutionResult, Self::State>, Self::Error> {
        let mut handler = MyHandler::default();
        handler.run(self).map(|result| {
            let state = self.finalize();
            ExecResultAndState::new(result, state)
        })
    }
}

// --- Simulation Manager ---
#[derive(Clone, Debug)]
pub struct RevmSimulator {
    // In future: add inspector, config, etc.
}

impl RevmSimulator {
    pub fn new() -> Self {
        Self {}
    }

    /// Stateless simulation of a transaction (no state commit)
    /// Accepts sender address, nonce, and balance to preload into the DB.
    /// Optionally, preload contract code for the 'to' address for event log emission.
    pub fn simulate_stateless_with_state(
        &self,
        tx_env: TxEnv,
        sender: revm::primitives::Address,
        sender_nonce: u64,
        sender_balance: revm::primitives::U256,
        to: Option<revm::primitives::Address>,
        contract_code: Option<Vec<u8>>,
    ) -> Result<SimResult> {
        let mut db = InMemoryDB::default();
        // Preload sender state
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: sender_balance,
                nonce: sender_nonce,
                code_hash: revm::primitives::keccak256(&[]),
                code: None,
            },
        );
        // Preload contract code if provided
        if let (Some(to_addr), Some(code_bytes)) = (to, contract_code) {
            let code_bytes = revm::primitives::Bytes::from(code_bytes);
            db.insert_account_info(
                to_addr,
                AccountInfo {
                    balance: revm::primitives::U256::ZERO, // Optionally fetch real balance
                    nonce: 0,
                    code_hash: keccak256(&code_bytes),
                    code: Some(Bytecode::new_raw(code_bytes)),
                },
            );
        }
        let ctx = Context::mainnet().with_db(db);
        let mut my_evm = MyEvm::new(ctx, ());
        let result = my_evm.transact_one(tx_env);
        let mut logs = vec![];
        let mut status = "unknown".to_string();
        let mut gas_used = 0u64;
        let mut output = None;
        match result {
            Ok(revm::context_interface::result::ExecutionResult::Success {
                gas_used: g,
                output: out,
                logs: ev_logs,
                ..
            }) => {
                status = "success".to_string();
                gas_used = g;
                output = match out {
                    Output::Call(data) => Some(data.to_vec()),
                    Output::Create(_, Some(data)) => Some(data.to_vec()),
                    _ => None,
                };
                for log in ev_logs {
                    logs.push(SimLog {
                        address: log.address.0.to_vec(),
                        topics: vec![], // TODO: REVM log.topics is private; cannot extract topics until REVM exposes them
                        data: format!("{:?}", log.data).into_bytes(), // TODO: log.data is private; cannot extract raw bytes until REVM exposes them
                    });
                }
            }
            Ok(revm::context_interface::result::ExecutionResult::Revert {
                gas_used: g,
                output: out,
            }) => {
                status = "revert".to_string();
                gas_used = g;
                output = Some(out.to_vec());
            }
            Ok(revm::context_interface::result::ExecutionResult::Halt {
                reason,
                gas_used: g,
                ..
            }) => {
                status = format!("halt: {:?}", reason);
                gas_used = g;
            }
            Err(e) => {
                status = format!("error: {:?}", e);
            }
        }
        Ok(SimResult {
            status,
            gas_used,
            output,
            logs,
        })
    }

    // Old version for backward compatibility (will be removed soon)
    pub fn simulate_stateless(&self, tx_env: TxEnv) -> Result<SimResult> {
        // For backward compatibility, just call with zero state
        self.simulate_stateless_with_state(
            tx_env,
            revm::primitives::Address::ZERO,
            0,
            revm::primitives::U256::ZERO,
            None,
            None,
        )
    }

    // --- Future: Inspector/Tracing support ---
    // pub fn simulate_with_tracing(&self, tx_env: TxEnv, inspector: impl Inspector<...>) -> ...

    pub async fn simulate(&self, tx: &ethers::types::Transaction) -> anyhow::Result<String> {
        // TODO: Implement actual simulation logic
        Ok("Simulation not yet implemented".to_string())
    }

    /// Simulate a transaction with full internal call tracing using MyTracer.
    pub fn simulate_with_trace(
        &self,
        tx_env: TxEnv,
        sender: revm::primitives::Address,
        sender_nonce: u64,
        sender_balance: revm::primitives::U256,
        to: Option<revm::primitives::Address>,
        contract_code: Option<Vec<u8>>,
    ) -> anyhow::Result<Option<CallTraceNode>> {
        let mut db = InMemoryDB::default();
        // Preload sender state
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: sender_balance,
                nonce: sender_nonce,
                code_hash: revm::primitives::keccak256(&[]),
                code: None,
            },
        );
        // Preload contract code if provided
        if let (Some(to_addr), Some(code_bytes)) = (to, contract_code) {
            let code_bytes = revm::primitives::Bytes::from(code_bytes);
            db.insert_account_info(
                to_addr,
                AccountInfo {
                    balance: revm::primitives::U256::ZERO, // Optionally fetch real balance
                    nonce: 0,
                    code_hash: keccak256(&code_bytes),
                    code: Some(Bytecode::new_raw(code_bytes)),
                },
            );
        }
        let ctx = Context::mainnet().with_db(db);
        let mut tracer = MyTracer::default();
        let mut my_evm = MyEvm::new(ctx, &mut tracer);
        my_evm.ctx().set_tx(tx_env);
        let mut handler = MyHandler::default();
        let _ = handler.inspect_run(&mut my_evm);
        Ok(tracer.root)
    }
    /// Pro-level: Simulate a transaction with full state forking using AlloyDB (live chain state).
    /// All contract code, storage, balances, etc. are fetched live from the node.
    /// provider_url: HTTP/WS endpoint of your BSC node (e.g. http://localhost:8545)
    /// This is the recommended forking pattern as per REVM examples.
    ///
    pub async fn simulate_with_forked_state(
        &self,
        tx_env: TxEnv,
        provider: Arc<DynProvider>,
    ) -> anyhow::Result<Option<CallTraceNode>> {
        // 1. Setup alloy provider
        // let provider: DynProvider = ProviderBuilder::new().connect(provider_url).await?.erased();
        // 1.1 Fetch block number from provider
        // let block_number = provider.get_block_number().await?;
        // println!("[DEBUG] Forked state at block number: {}", block_number);
        // 2. Setup AlloyDB (forking DB) at this block
        // let block_id = BlockId::Number(block_number.into());
        // println!("[DEBUG] Using BlockId for fork: {:?}", block_id);
        let alloy_db =
            WrapDatabaseAsync::new(AlloyDB::new((provider).as_ref().clone(), BlockId::latest()))
                .unwrap();
        let mut cache_db = CacheDB::new(alloy_db);
        // --- Debug: Print contract code length for 'to' address ---
        if let Some(to_addr) = match &tx_env.kind {
            revm::primitives::TxKind::Call(addr) => Some(*addr),
            _ => None,
        } {
            let acc_info = cache_db.basic(to_addr)?;
            // println!("[DEBUG] Fetched code len for to: 0x{} => {}", hex::encode(to_addr), acc_info.as_ref().and_then(|info| info.code.as_ref()).map(|c| c.len()).unwrap_or(0));
            let code_len = acc_info
                .as_ref()
                .and_then(|info| info.code.as_ref())
                .map(|c| c.len())
                .unwrap_or(0);
            // println!("[DEBUG] Fetched code len for to: 0x{} => {}", hex::encode(to_addr), code_len);
            if code_len == 0 {
                // println!("[DEBUG] WARNING: No contract code found for to: 0x{}!", hex::encode(to_addr));
            }
        }
        // --- Debug: Print TxEnv fields ---
        // println!("[DEBUG] TxEnv: gas_limit: {}, gas_price: {}, value: {}, nonce: {}, input_len: {}", tx_env.gas_limit, tx_env.gas_price, tx_env.value, tx_env.nonce, tx_env.data.len());
        // 3. Setup REVM context with CacheDB
        let mut ctx = Context::mainnet().with_db(cache_db);
        ctx.cfg.disable_nonce_check = true;
        // Print current block number for debug
        // println!("[DEBUG] Simulating at block number: {}", ctx.block.number);
        // 4. Setup EVM (MyEvm or direct)
        let mut tracer = MyTracer::default();
        let mut my_evm = MyEvm::new(ctx, &mut tracer);
        my_evm.ctx().set_tx(tx_env);
        // 5. Run simulation with inspector/tracer for full call trace
        let mut handler = MyHandler::default();
        let result = handler.inspect_run(&mut my_evm);
        // println!("[DEBUG] Simulation result: {:?}", result);
        // 6. tracer.root me full call trace tree hai
        // Optionally, pretty-print:
        // if let Some(ref root) = tracer.root {
        //     print_full_call_trace(root, 0);
        // }
        // 7. Optionally, extract logs/events from call trace
        // (You can walk tracer.root to collect all logs/events)
        Ok(tracer.root)
    }

    /// Ultra-low-latency: Simulate a transaction using a preloaded RAM-only CacheDB (no network I/O).
    /// This is the recommended path for MEV/mempool bots after state warmup.
    pub fn simulate_with_preloaded_cache(
        &self,
        tx_env: TxEnv,
        cache_db: &CacheDB<EmptyDB>,
    ) -> anyhow::Result<Option<CallTraceNode>> {
        use revm::Context;
        // Clone the cache_db for thread safety (each simulation gets its own snapshot)
        let mut cache_db = cache_db.clone();
        // --- Debug: Print contract code length for 'to' address ---
        if let Some(to_addr) = match &tx_env.kind {
            revm::primitives::TxKind::Call(addr) => Some(*addr),
            _ => None,
        } {
            let acc_info = cache_db.basic(to_addr)?;
            let code_len = acc_info
                .as_ref()
                .and_then(|info| info.code.as_ref())
                .map(|c| c.len())
                .unwrap_or(0);
            println!(
                "[DEBUG] [RAM] Fetched code len for to: 0x{} => {}",
                hex::encode(to_addr),
                code_len
            );
            if code_len == 0 {
                println!(
                    "[DEBUG] [RAM] WARNING: No contract code found for to: 0x{}!",
                    hex::encode(to_addr)
                );
            }
        }
        // --- Debug: Print TxEnv fields ---
        println!(
            "[DEBUG] [RAM] TxEnv: gas_limit: {}, gas_price: {}, value: {}, nonce: {}, input_len: {}",
            tx_env.gas_limit,
            tx_env.gas_price,
            tx_env.value,
            tx_env.nonce,
            tx_env.data.len()
        );
        // 1. Setup REVM context with RAM-only CacheDB
        let mut ctx = Context::mainnet().with_db(cache_db);
        ctx.cfg.disable_nonce_check = true;
        // 2. Setup EVM and tracer
        let mut tracer = MyTracer::default();
        let mut my_evm = MyEvm::new(ctx, &mut tracer);
        my_evm.ctx().set_tx(tx_env);
        // 3. Run simulation with inspector/tracer
        let mut handler = MyHandler::default();
        let result = handler.inspect_run(&mut my_evm);
        println!("[DEBUG] [RAM] Simulation result: {:?}", result);
        Ok(tracer.root)
    }

    /*
    // Example usage:
    let tx_env = ...; // Build TxEnv from ethers tx
    let sim = RevmSimulator::new();
    let sim_result = sim.simulate_with_forked_state(tx_env, "http://localhost:8545")?;
    print_simresult_logs(&sim_result);
    */
}

/// Pretty-print the call trace tree recursively (public for pipeline use)
pub fn print_full_call_trace(node: &CallTraceNode, indent: usize) {
    let pad = "  ".repeat(indent);
    println!(
        "{}Call: {} from 0x{} to 0x{} value {} input {}",
        pad,
        node.call_type,
        hex::encode(node.from),
        hex::encode(node.to),
        node.value,
        hex::encode(&node.input)
    );
    if let Some(output) = &node.output {
        println!("{}  Output: {}", pad, hex::encode(output));
    }
    for (i, log) in node.logs.iter().enumerate() {
        // Try to parse topics/data from debug string if possible
        let (topics, data_hex) = parse_logdata_string(&log.data);
        println!(
            "{}  Log #{}: address 0x{}",
            pad,
            i,
            hex::encode(log.address)
        );
        for (j, topic) in topics.iter().enumerate() {
            println!("{}    topics[{}]: {}", pad, j, topic);
        }
        println!("{}    data: {}", pad, data_hex);
    }
    for child in &node.children {
        print_full_call_trace(child, indent + 1);
    }
}

// --- Internal Call Trace Tracer ---
// use revm::inspector::Inspector;
use revm::interpreter::Interpreter;
use revm::primitives::{Address, Bytes, Log};

#[derive(Debug, Clone)]
pub struct CallTraceNode {
    pub call_type: String,
    pub from: Address,
    pub to: Address,
    pub value: B256,
    pub input: Bytes,
    pub output: Option<Bytes>,
    pub depth: usize,
    pub children: Vec<CallTraceNode>,
    pub logs: Vec<TraceLog>,
}

#[derive(Debug, Clone)]
pub struct TraceLog {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
}

#[derive(Debug, Default)]
pub struct MyTracer {
    pub root: Option<CallTraceNode>,
    pub current_stack: Vec<CallTraceNode>,
}
// use revm::interpreter::Interpreter;
// use revm::primitives::{Address, Bytes,  U256, Log};
// Make Inspector implementation generic over DB
impl<DB>
    revm::inspector::Inspector<
        revm::Context<BlockEnv, TxEnv, CfgEnv, DB>,
        revm::interpreter::interpreter::EthInterpreter,
    > for MyTracer
where
    DB: revm::context_interface::Database,
{
    fn call(
        &mut self,
        ctx: &mut revm::Context<BlockEnv, TxEnv, CfgEnv, DB>,
        inputs: &mut revm::interpreter::CallInputs,
    ) -> Option<revm::interpreter::CallOutcome> {
        // println!("[TRACER] call: from 0x{} to 0x{} value {:?} input_len {}", hex::encode(inputs.caller), hex::encode(inputs.target_address), inputs.value, inputs.input.len());
        let node = CallTraceNode {
            call_type: format!("{:?}", inputs.scheme),
            from: inputs.caller,
            to: inputs.target_address,
            value: inputs.value.get().into(),
            input: inputs.input.bytes(ctx),
            output: None,
            depth: self.current_stack.len(),
            children: vec![],
            logs: vec![],
        };
        self.current_stack.push(node);
        None
    }

    fn call_end(
        &mut self,
        _ctx: &mut revm::Context<BlockEnv, TxEnv, CfgEnv, DB>,
        _inputs: &revm::interpreter::CallInputs,
        outcome: &mut revm::interpreter::CallOutcome,
    ) {
        if let Some(mut node) = self.current_stack.pop() {
            node.output = Some(outcome.output().clone());
            if let Some(parent) = self.current_stack.last_mut() {
                parent.children.push(node);
            } else {
                self.root = Some(node);
            }
        }
    }

    fn log(
        &mut self,
        _interp: &mut Interpreter,
        _ctx: &mut revm::Context<BlockEnv, TxEnv, CfgEnv, DB>,
        log: Log,
    ) {
        // Store actual log data bytes for decoding
        if let Some(node) = self.current_stack.last_mut() {
            node.logs.push(TraceLog {
                address: log.address,
                topics: vec![], // not used
                data: Bytes::from(format!("{:?}", log).into_bytes()),
            });
        }
    }
}

/// Pretty-print the call trace tree recursively
pub fn print_call_trace(node: &CallTraceNode, indent: usize) {
    let pad = "  ".repeat(indent);
    println!(
        "{}Call: {} from 0x{} to 0x{} value {} input {}",
        pad,
        node.call_type,
        hex::encode(node.from),
        hex::encode(node.to),
        node.value,
        hex::encode(&node.input)
    );
    if let Some(output) = &node.output {
        println!("{}  Output: {}", pad, hex::encode(output));
    }
    for (i, log) in node.logs.iter().enumerate() {
        println!(
            "{}  Log #{}: address 0x{} topics {:?} data {}",
            pad,
            i,
            hex::encode(log.address),
            log.topics
                .iter()
                .map(|t| format!("0x{}", hex::encode(t)))
                .collect::<Vec<_>>(),
            hex::encode(&log.data)
        );
    }
    for child in &node.children {
        print_call_trace(child, indent + 1);
    }
}

pub static DEX_EVENT_TOPICS: Lazy<HashSet<B256>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert(B256::from_slice(
        keccak256("Swap(address,address,uint256,uint256,uint256,uint256,address)").as_slice(),
    ));
    set.insert(B256::from_slice(
        keccak256("Swap(address,uint256,uint256,uint256,uint256,address)").as_slice(),
    ));
    set.insert(B256::from_slice(
        keccak256("Sync(uint112,uint112)").as_slice(),
    ));
    set.insert(B256::from_slice(
        keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)").as_slice(),
    ));
    set.insert(B256::from_slice(
        keccak256("Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)")
            .as_slice(),
    ));
    set
});

static SWAP_V2_BROADCAST: Lazy<broadcast::Sender<String>> = Lazy::new(|| {
    // 1024 message buffer
    let (tx, _rx) = broadcast::channel(1024);
    tx
});

pub async fn start_ipc_broadcast(path: &str) {
    use tokio::io::AsyncWriteExt;
    let listener = UnixListener::bind(path).expect("Failed to bind IPC socket");
    let mut rx = SWAP_V2_BROADCAST.subscribe();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _addr)) => {
                    let mut rx = SWAP_V2_BROADCAST.subscribe();
                    tokio::spawn(async move {
                        while let Ok(msg) = rx.recv().await {
                            let _ = stream.write_all(msg.as_bytes()).await;
                            let _ = stream.write_all(b"\n").await;
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[IPC] Accept error: {:?}", e);
                }
            }
        }
    });
}

fn decode_and_print_swap_v2(data_hex: &str, pool: H160, reserve_cache: &Arc<ReserveCache>) {
    if let Ok(data_bytes) = hex::decode(data_hex.trim_start_matches("0x")) {
        let param_types = vec![
            ParamType::Uint(256), // amount0In
            ParamType::Uint(256), // amount1In
            ParamType::Uint(256), // amount0Out
            ParamType::Uint(256), // amount1Out
        ];
        if let Ok(tokens) = ethers::abi::decode(&param_types, &data_bytes) {
            let amount0_in = tokens[0].clone().into_uint().unwrap();
            let amount1_in = tokens[1].clone().into_uint().unwrap();
            let amount0_out = tokens[2].clone().into_uint().unwrap();
            let amount1_out = tokens[3].clone().into_uint().unwrap();
            println!("      amount0In:   {}", amount0_in);
            println!("      amount1In:  {}", amount1_in);
            println!("      amount0Out: {}", amount0_out);
            println!("      amount1Out: {}", amount1_out);
            // Optionally update cache here if needed (e.g. for swap volume tracking)
        }
    }
}
use std::io::Write;
async fn decode_and_print_sync_v2(
    data_hex: &str,
    pool: H160,
    reserve_cache: &Arc<ReserveCache>,
    block_number: u64,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
) {
    if let Ok(data_bytes) = hex::decode(data_hex.trim_start_matches("0x")) {
        let param_types = vec![
            ParamType::Uint(112), // reserve0
            ParamType::Uint(112), // reserve1
        ];
        if let Ok(tokens) = ethers::abi::decode(&param_types, &data_bytes) {
            let new_reserve0 = tokens[0].clone().into_uint().unwrap();
            let new_reserve1 = tokens[1].clone().into_uint().unwrap();
            println!("      reserve0:   {}", new_reserve0);
            println!("      reserve1:   {}", new_reserve1);
            // --- CACHE UPDATE ---
            // Get old reserves before updating
            let old_reserve0 = reserve_cache
                .get(&pool)
                .and_then(|s| s.reserve0)
                .unwrap_or(eU256::zero());
            let old_reserve1 = reserve_cache
                .get(&pool)
                .and_then(|s| s.reserve1)
                .unwrap_or(eU256::zero());
            
            // Print cache state BEFORE update
            println!("      [CACHE BEFORE] Pool: {:?}", pool);
            println!("      [CACHE BEFORE] Old reserve0: {}", old_reserve0);
            println!("      [CACHE BEFORE] Old reserve1: {}", old_reserve1);
            println!("      [CACHE BEFORE] New reserve0: {}", new_reserve0);
            println!("      [CACHE BEFORE] New reserve1: {}", new_reserve1);
            
            if let Some(mut state) = reserve_cache.get_mut(&pool) {
                state.reserve0 = Some(new_reserve0);
                state.reserve1 = Some(new_reserve1);
                state.last_updated = chrono::Utc::now().timestamp() as u64;
                
                // Print cache state AFTER update
                println!("      [CACHE AFTER] Pool: {:?}", pool);
                println!("      [CACHE AFTER] Updated reserve0: {:?}", state.reserve0);
                println!("      [CACHE AFTER] Updated reserve1: {:?}", state.reserve1);
                println!("      [CACHE AFTER] Last updated: {}", state.last_updated);
                println!("      [CACHE UPDATE] ✅ SUCCESS - Reserves updated in cache!");
            } else {
                println!("      [CACHE UPDATE] ❌ FAILED - Pool not found in cache: {:?}", pool);
            }
            // Create decoded swap for arbitrage detection
            let (token_x, token_x_amount) = if new_reserve0 < old_reserve0 {
                // token0 bought (reserve0 decreased)
                if let Some(pool_data) = reserve_cache.get(&pool) {
                    (pool_data.token0, old_reserve0.saturating_sub(new_reserve0))
                } else {
                    return;
                }
            } else if new_reserve1 < old_reserve1 {
                // token1 bought (reserve1 decreased)
                if let Some(pool_data) = reserve_cache.get(&pool) {
                    (pool_data.token1, old_reserve1.saturating_sub(new_reserve1))
                } else {
                    return;
                }
            } else {
                return;
            };

            let decoded_swap = DecodedSwap {
                tx_hash: H160::zero(), // Sync events don't have direct tx hash
                pool_address: pool,
                token_x,
                token_x_amount,
                block_number,
                timestamp: chrono::Utc::now().timestamp() as u64,
            };
            println!("[DecodedSwap] {:?}", decoded_swap);

            // --- Start latency monitoring ---
            let t0 = Instant::now();
            let mut timings = serde_json::Map::new();
            timings.insert("search_start_us".to_string(), serde_json::json!(0));

            // --- Opportunity search (simulation/filtering) ---
            let after_sim;
            let before_tx;
            let after_tx;
            let mut tx_hash_str: Option<String> = None;
            if let Some((opportunity, latency_ms)) = find_arbitrage_opportunity_from_price_tracker(
                &decoded_swap,
                reserve_cache,
                token_index,
                precomputed_route_cache,
                token_tax_map,
                &config,
            )
            .await
            {
                after_sim = t0.elapsed().as_micros();
                timings.insert("after_sim_us".to_string(), serde_json::json!(after_sim));

                // Log the opportunity
                log_opportunity_from_price_tracker(
                    &opportunity,
                    latency_ms,
                    reserve_cache,
                    old_reserve0,
                    old_reserve1,
                );

                // --- Before TX fire ---
                before_tx = t0.elapsed().as_micros();
                timings.insert("before_tx_us".to_string(), serde_json::json!(before_tx));

                // --- Simulate TX fire (mock, replace with actual call if needed) ---
                // let tx_hash = execute_arbitrage_onchain(...).await?;
                // For now, just simulate delay
                // tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                // after_tx = t0.elapsed().as_micros();
                // timings.insert("after_tx_us".to_string(), serde_json::json!(after_tx));
                // timings.insert("tx_hash".to_string(), serde_json::json!(tx_hash.to_string()));

                // Send opportunity for execution
                if let Err(e) = opportunity_tx.send(opportunity).await {
                    eprintln!(
                        "❌ [Price Tracker] Failed to send arbitrage opportunity: {}",
                        e
                    );
                }
                after_tx = t0.elapsed().as_micros();
                timings.insert("after_tx_us".to_string(), serde_json::json!(after_tx));
                timings.insert("tx_hash".to_string(), serde_json::json!(tx_hash_str));

                // --- Total ---
                let total = t0.elapsed().as_millis();
                timings.insert("total_ms".to_string(), serde_json::json!(total));

                // Print and log timings
                // println!("[LATENCY] Step timings: {}", serde_json::to_string_pretty(&timings).unwrap());
                // Optionally, append to a timings log file
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("latency_breakdown_price_tracker.log")
                {
                    if let Err(e) = writeln!(file, "{}", serde_json::to_string(&timings).unwrap()) {
                        eprintln!("❌ [Price Tracker] Failed to write latency log: {}", e);
                    }
                }
            }
        }
    }
}
use num_traits::ToPrimitive;
fn decode_and_print_swap_v3(data_hex: &str, pool: H160, reserve_cache: &Arc<ReserveCache>) {
    use ethers::abi::Token;
    use num_bigint::BigInt;
    if let Ok(data_bytes) = hex::decode(data_hex.trim_start_matches("0x")) {
        let param_types = vec![
            ParamType::Int(256),  // amount0 (signed!)
            ParamType::Int(256),  // amount1 (signed!)
            ParamType::Uint(160), // sqrtPriceX96
            ParamType::Uint(128), // liquidity
            ParamType::Int(24),   // tick (signed!)
        ];
        if let Ok(tokens) = ethers::abi::decode(&param_types, &data_bytes) {
            let sqrt_price_x96 = tokens[2].clone().into_uint().unwrap();
            let liquidity = tokens[3].clone().into_uint().unwrap();
            let tick = match &tokens[4] {
                Token::Int(i) => {
                    let mut buf = [0u8; 32];
                    i.to_big_endian(&mut buf);
                    BigInt::from_signed_bytes_be(&buf)
                }
                _ => panic!("not int"),
            };
            let tick_i32: i32 = tick.to_i32().expect("tick value out of range for i32");
            println!("      sqrtPriceX96: {}", sqrt_price_x96);
            println!("      liquidity:    {}", liquidity);
            println!("      tick:         {}", tick);
            // --- CACHE UPDATE ---
            // Get old values before updating
            let old_sqrt_price_x96 = reserve_cache
                .get(&pool)
                .and_then(|s| s.sqrt_price_x96)
                .unwrap_or(eU256::zero());
            let old_liquidity = reserve_cache
                .get(&pool)
                .and_then(|s| s.liquidity)
                .unwrap_or(eU256::zero());
            let old_tick = reserve_cache
                .get(&pool)
                .and_then(|s| s.tick)
                .unwrap_or(0i32);
            
            // Print cache state BEFORE update
            println!("      [CACHE BEFORE] Pool: {:?}", pool);
            println!("      [CACHE BEFORE] Old sqrtPriceX96: {}", old_sqrt_price_x96);
            println!("      [CACHE BEFORE] Old liquidity: {}", old_liquidity);
            println!("      [CACHE BEFORE] Old tick: {}", old_tick);
            println!("      [CACHE BEFORE] New sqrtPriceX96: {}", sqrt_price_x96);
            println!("      [CACHE BEFORE] New liquidity: {}", liquidity);
            println!("      [CACHE BEFORE] New tick: {}", tick_i32);
            
            if let Some(mut state) = reserve_cache.get_mut(&pool) {
                state.sqrt_price_x96 = Some(sqrt_price_x96);
                state.liquidity = Some(liquidity);
                state.tick = Some(tick_i32);
                state.last_updated = chrono::Utc::now().timestamp() as u64;
                
                // Print cache state AFTER update
                println!("      [CACHE AFTER] Pool: {:?}", pool);
                println!("      [CACHE AFTER] Updated sqrtPriceX96: {:?}", state.sqrt_price_x96);
                println!("      [CACHE AFTER] Updated liquidity: {:?}", state.liquidity);
                println!("      [CACHE AFTER] Updated tick: {:?}", state.tick);
                println!("      [CACHE AFTER] Last updated: {}", state.last_updated);
                println!("      [CACHE UPDATE] ✅ SUCCESS - V3 state updated in cache!");
            } else {
                println!("      [CACHE UPDATE] ❌ FAILED - V3 Pool not found in cache: {:?}", pool);
            }
            
        }
    }
}

fn decode_and_print_pancake_swap_v3(
    data_hex: &str,
    topics: &[String],
    pool: H160,
    reserve_cache: &Arc<ReserveCache>,
) {
    use ethers::abi::{ParamType, Token};
    use num_bigint::BigInt;
    if let Ok(data_bytes) = hex::decode(data_hex.trim_start_matches("0x")) {
        let param_types = vec![
            ParamType::Int(256),  // amount0
            ParamType::Int(256),  // amount1
            ParamType::Uint(160), // sqrtPriceX96
            ParamType::Uint(128), // liquidity
            ParamType::Int(24),   // tick
            ParamType::Uint(128), // protocolFeesToken0
            ParamType::Uint(128), // protocolFeesToken1
        ];
        if let Ok(tokens) = ethers::abi::decode(&param_types, &data_bytes) {
            let sqrt_price_x96 = tokens[2].clone().into_uint().unwrap();
            let liquidity = tokens[3].clone().into_uint().unwrap();
            let tick = match &tokens[4] {
                Token::Int(i) => {
                    let mut buf = [0u8; 32];
                    i.to_big_endian(&mut buf);
                    BigInt::from_signed_bytes_be(&buf)
                }
                _ => panic!("not int"),
            };
            println!("      sqrtPriceX96: {}", sqrt_price_x96);
            println!("      liquidity:   {}", liquidity);
            println!("      tick:        {}", tick);
            let tick_i32: i32 = tick.to_i32().expect("tick value out of range for i32");
            // --- CACHE UPDATE ---
            // Get old values before updating
            let old_sqrt_price_x96 = reserve_cache
                .get(&pool)
                .and_then(|s| s.sqrt_price_x96)
                .unwrap_or(eU256::zero());
            let old_liquidity = reserve_cache
                .get(&pool)
                .and_then(|s| s.liquidity)
                .unwrap_or(eU256::zero());
            let old_tick = reserve_cache
                .get(&pool)
                .and_then(|s| s.tick)
                .unwrap_or(0i32);
            
            // Print cache state BEFORE update
            println!("      [CACHE BEFORE] Pool: {:?}", pool);
            println!("      [CACHE BEFORE] Old sqrtPriceX96: {}", old_sqrt_price_x96);
            println!("      [CACHE BEFORE] Old liquidity: {}", old_liquidity);
            println!("      [CACHE BEFORE] Old tick: {}", old_tick);
            println!("      [CACHE BEFORE] New sqrtPriceX96: {}", sqrt_price_x96);
            println!("      [CACHE BEFORE] New liquidity: {}", liquidity);
            println!("      [CACHE BEFORE] New tick: {}", tick_i32);
            
            if let Some(mut state) = reserve_cache.get_mut(&pool) {
                state.sqrt_price_x96 = Some(sqrt_price_x96);
                state.liquidity = Some(liquidity);
                state.tick = Some(tick_i32);
                state.last_updated = chrono::Utc::now().timestamp() as u64;
                
                // Print cache state AFTER update
                println!("      [CACHE AFTER] Pool: {:?}", pool);
                println!("      [CACHE AFTER] Updated sqrtPriceX96: {:?}", state.sqrt_price_x96);
                println!("      [CACHE AFTER] Updated liquidity: {:?}", state.liquidity);
                println!("      [CACHE AFTER] Updated tick: {:?}", state.tick);
                println!("      [CACHE AFTER] Last updated: {}", state.last_updated);
                println!("      [CACHE UPDATE] ✅ SUCCESS - V3 state updated in cache!");
            } else {
                println!("      [CACHE UPDATE] ❌ FAILED - V3 Pool not found in cache: {:?}", pool);
            }
        }
    }
}

use std::future::Future;
use std::pin::Pin;

pub fn print_dex_events_from_trace<'a>(
    node: &'a CallTraceNode,
    tx_hash: &'a str,
    reserve_cache: &'a Arc<ReserveCache>,
    token_index: &'a Arc<TokenIndexMap>,
    precomputed_route_cache: &'a Arc<DashMap<u32, Vec<RoutePath>>>,
    token_tax_map: &'a Arc<TokenTaxMap>,
    config: &'a Config,
    opportunity_tx: &'a mpsc::Sender<ArbitrageOpportunity>,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        for log in &node.logs {
            let (topics, data_hex) = parse_logdata_string2(&log.data);
            let pool = H160::from_slice(log.address.0.as_slice());
            if let Some(topic0) = topics.get(0) {
                if let Ok(topic0_bytes) = hex::decode(topic0.trim_start_matches("0x")) {
                    if DEX_EVENT_TOPICS.contains(&B256::from_slice(&topic0_bytes)) {
                        let event_name = match topic0.as_str() {
                            t if t
                                == format!(
                                    "0x{:x}",
                                    keccak256(
                                        "Swap(address,address,uint256,uint256,uint256,uint256,address)"
                                    )
                                ) =>
                            {
                                "SwapV2"
                            }
                            t if t
                                == format!(
                                    "0x{:x}",
                                    keccak256("Swap(address,uint256,uint256,uint256,uint256,address)")
                                ) =>
                            {
                                "SwapV2"
                            }
                            t if t == format!("0x{:x}", keccak256("Sync(uint112,uint112)")) => "SyncV2",
                            t if t
                                == format!(
                                    "0x{:x}",
                                    keccak256(
                                        "Swap(address,address,int256,int256,uint160,uint128,int24)"
                                    )
                                ) =>
                            {
                                "SwapV3"
                            }
                            t if t
                                == format!(
                                    "0x{:x}",
                                    keccak256(
                                        "Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)"
                                    )
                                ) =>
                            {
                                "PanCakeSwapV3"
                            }
                            _ => "UnknownDEXEvent",
                        };
                        println!(
                            "[DEX EVENT] {} at 0x{} (tx: {})",
                            event_name,
                            hex::encode(&log.address),
                            tx_hash
                        );
                        match event_name {
                            "SwapV2" => {
                                println!("      [DEBUG] log struct: {:?}", log);
                                decode_and_print_swap_v2(&data_hex, pool, reserve_cache);
                            }
                            "SyncV2" => {
                                println!("      [DEBUG] topocs  : {:?}", topics);
                                decode_and_print_sync_v2(
                                    &data_hex,
                                    pool,
                                    reserve_cache,
                                    0, // You may need to provide the correct block_number value here
                                    token_index,
                                    precomputed_route_cache,
                                    token_tax_map,
                                    config,
                                    opportunity_tx,
                                ).await;
                            }
                            "SwapV3" => {
                                println!("      [DEBUG] topocs  : {:?}", topics);
                                decode_and_print_swap_v3(&data_hex, pool, reserve_cache);
                            }
                            "PanCakeSwapV3" => {
                                println!("      [DEBUG] topics  : {:?}", topics);
                                decode_and_print_pancake_swap_v3(
                                    &data_hex,
                                    &topics,
                                    pool,
                                    reserve_cache,
                                );
                            }
                            _ => println!("      raw data: {}", data_hex),
                        }
                    }
                }
            }
        }
        for child in &node.children {
            print_dex_events_from_trace(
                child,
                tx_hash,
                reserve_cache,
                token_index,
                precomputed_route_cache,
                token_tax_map,
                config,
                opportunity_tx,
            ).await;
        }
    })
}
/// Find arbitrage opportunities for a decoded swap (price tracker version)
pub async fn find_arbitrage_opportunity_from_price_tracker(
    decoded_swap: &DecodedSwap,
    reserve_cache: &Arc<ReserveCache>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &Config,
) -> Option<(ArbitrageOpportunity, u128)> {
    // Start latency timer
    let start_time = std::time::Instant::now();
    // Get token index
    let token_x_index = token_index.address_to_index.get(&decoded_swap.token_x)?;
    let token_x_index_u32 = *token_x_index as u32;

    println!(
        "🔍 [Price Tracker] Finding arbitrage for tokenX (idx {}): {:?}",
        token_x_index, decoded_swap.token_x
    );

    // Get all routes that contain this token and the affected pool
    let candidate_routes = precomputed_route_cache
        .get(&token_x_index_u32)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();

    // println!(
    //     "📊 [Price Tracker] Found {} candidate routes for tokenX",
    //     candidate_routes.len()
    // );

    // Filter routes that contain the affected pool
    let filtered_routes: Vec<&RoutePath> = candidate_routes
        .iter()
        .filter(|route| route.pools.contains(&decoded_swap.pool_address))
        .collect();

    println!(
        "🎯 [Price Tracker] {} routes contain the affected pool {:?}",
        filtered_routes.len(),
        decoded_swap.pool_address
    );

    if filtered_routes.is_empty() {
        return None;
    }

    // Simulate all filtered routes in parallel
    let simulation_results: Vec<Option<crate::arbitrage_finder::SimulatedRoute>> = filtered_routes
        .par_iter()
        .map(|route| {
            // Split route into buy/sell paths
            let (buy_path, sell_path) = split_route_around_token_x(route, token_x_index_u32)?;

            // Simulate buy path (base -> tokenX)
            let buy_amounts = simulate_buy_path_amounts_array(
                &buy_path,
                decoded_swap.token_x_amount,
                reserve_cache,
                token_index,
                token_tax_map,
                config,
            )?;

            // Simulate sell path (tokenX -> base)
            let sell_amounts = simulate_sell_path_amounts_array(
                &sell_path,
                decoded_swap.token_x_amount,
                reserve_cache,
                token_index,
                token_tax_map,
                config,
            )?;

            // Merge amounts: [buy_amounts..., sell_amounts[1..]]
            let mut merged_amounts = buy_amounts.clone();
            merged_amounts.extend_from_slice(&sell_amounts[1..]);
            // let sell_test_amounts;
            // simulate_sell_path_amounts_array(
            //     route,
            //     merged_amounts[0],
            //     reserve_cache,
            //     token_index,
            // )?;
            // Calculate profit and profit percentage
            if merged_amounts.len() >= 2 {
                let amount_in: eU256 = merged_amounts[0];
                let amount_out: eU256 = *merged_amounts.last().unwrap();
                let profit: eU256 = amount_out.saturating_sub(amount_in);

                // Only consider profitable trades
                let sell_symbols: Vec<String> = sell_path
                    .hops
                    .iter()
                    .map(|&idx| token_index_to_symbol_from_price_tracker(idx, token_index))
                    .collect();
                let price_usd = {
                    let last_symbol = &sell_symbols[sell_symbols.len() - 1];
                    if let Ok(addr) = last_symbol.parse::<H160>() {
                        get_token_usd_value(&addr).unwrap_or(0.0)
                    } else {
                        0.0
                    }
                };
                let amount = u256_to_f64_lossy(&profit) / 10_f64.powi(18 as i32);
                let profit_usd = amount * price_usd;
                if amount_in < amount_out {
                    // Calculate profit percentage (profit / amount_in * 100)
                    let profit_percentage = if amount_in > eU256::zero() {
                        // Convert to f64 for percentage calculation
                        let profit_f64 = profit.as_u128() as f64;
                        let amount_in_f64 = amount_in.as_u128() as f64;
                        (profit_f64 / amount_in_f64) * 100.0
                    } else {
                        0.0
                    };

                    // Merge token indices
                    // let mut merged_tokens = buy_path.hops.clone();
                    // merged_tokens.extend_from_slice(&sell_path.hops[1..]);

                    // Map to symbols
                    // let merged_symbols = merged_tokens
                    //     .iter()
                    //     .map(|&idx| token_index_to_symbol_from_price_tracker(idx, token_index))
                    //     .collect();

                    // Merge pools
                    let mut merged_pools = buy_path.pools.clone();
                    merged_pools.extend_from_slice(&sell_path.pools);

                    return Some(crate::arbitrage_finder::SimulatedRoute {
                        merged_amounts,
                        buy_amounts,
                        sell_amounts,
                        // merged_tokens,
                        // merged_symbols,
                        buy_symbols: buy_path
                            .hops
                            .iter()
                            .map(|&idx| token_index_to_symbol_from_price_tracker(idx, token_index))
                            .collect(),
                        sell_symbols,
                        buy_pools: buy_path.pools.clone(),
                        sell_pools: sell_path.pools.clone(),
                        merged_pools,
                        profit,
                        profit_percentage,
                        buy_path: buy_path.clone(),
                        sell_path: sell_path.clone(),
                        // sell_test_amounts,
                    });
                }
            }

            None
        })
        .collect();

    // Filter out None results
    let profitable_routes: Vec<crate::arbitrage_finder::SimulatedRoute> =
        simulation_results.into_iter().filter_map(|r| r).collect();

    println!(
        "💰 [Price Tracker] Found {} profitable routes",
        profitable_routes.len()
    );

    if profitable_routes.is_empty() {
        return None;
    }

    // Find the most profitable route by percentage (better for multiple base tokens)
    let best_route = profitable_routes
        .iter()
        .max_by(|a, b| {
            a.profit_percentage
                .partial_cmp(&b.profit_percentage)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned();

    let estimated_profit = best_route
        .as_ref()
        .map(|r| r.profit)
        .unwrap_or(eU256::zero());

    // End latency timer
    let latency = start_time.elapsed().as_millis();

    Some((
        ArbitrageOpportunity {
            decoded_swap: decoded_swap.clone(),
            profitable_routes,
            best_route,
            estimated_profit,
        },
        latency,
    ))
}
fn u256_to_f64_lossy(val: &eU256) -> f64 {
    if val.bits() <= 128 {
        val.as_u128() as f64
    } else {
        val.to_string().parse::<f64>().unwrap_or(f64::MAX)
    }
}
const KNOWN_TOKENS: &[(&str, &str, f64)] = &[
    ("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", "BNB", 689.93),
    ("0x2170Ed0880ac9A755fd29B2688956BD959F933F8", "ETH", 2961.19),
    (
        "0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c",
        "BTC",
        117970.0,
    ),
    ("0x55d398326f99059fF775485246999027B3197955", "USDT", 1.00),
    ("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d", "USDC", 1.00), // Multichain bridge price
    ("0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56", "BUSD", 1.00),
    ("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82", "CAKE", 2.37),
];

fn get_token_usd_value(token_address: &H160) -> Option<f64> {
    let addr_str = format!("0x{:x}", token_address);
    KNOWN_TOKENS
        .iter()
        .find(|(addr, _, _)| addr.to_lowercase() == addr_str.to_lowercase())
        .map(|(_, _, price)| *price)
}
/// Helper to map token index to symbol (price tracker version)
fn token_index_to_symbol_from_price_tracker(idx: u32, token_index: &TokenIndexMap) -> String {
    if let Some(addr) = token_index.index_to_address.get(&(idx as u32)) {
        format!("0x{:x}", addr)
    } else {
        format!("token{}", idx)
    }
}

fn log_opportunity_from_price_tracker(
    opportunity: &ArbitrageOpportunity,
    latency_ms: u128,
    reserve_cache: &crate::cache::ReserveCache,
    old_reserve0: eU256,
    old_reserve1: eU256,
) {
    use crate::cache::PoolState;
    use ethers::types::U256;
    // Fetch current pool state from reserve_cache
    let (reserve0, reserve1, sqrt_price_x96, liquidity, tick, fee): (
        Option<U256>,
        Option<U256>,
        Option<U256>,
        Option<U256>,
        Option<i32>,
        Option<u32>,
    ) = {
        if let Some(state) = reserve_cache.get(&opportunity.decoded_swap.pool_address) {
            (
                state.reserve0,
                state.reserve1,
                state.sqrt_price_x96,
                state.liquidity,
                state.tick,
                state.fee,
            )
        } else {
            (None, None, None, None, None, None)
        }
    };

    let now: DateTime<Utc> = Utc::now();
    let log_file_path = format!(
        "logs/arbitrage_opportunities_price_tracker_{}.log",
        now.format("%Y%m%d_%H%M%S")
    );

    // Create detailed log entry
    let mut log_entry = json!({
        "source": "price_tracker",
        "timestamp": now.to_rfc3339(),
        "block_number": opportunity.decoded_swap.block_number,
        "pool_address": format!("0x{:x}", opportunity.decoded_swap.pool_address),
        "token_x": format!("0x{:x}", opportunity.decoded_swap.token_x),
        "token_x_amount": opportunity.decoded_swap.token_x_amount.to_string(),
        "estimated_profit": opportunity.estimated_profit.to_string(),
        "profitable_routes_count": opportunity.profitable_routes.len(),
        "latency_ms": latency_ms,
        "reserve0": reserve0.map(|v| v.to_string()),
        "reserve1": reserve1.map(|v| v.to_string()),
        "old_reserve0": old_reserve0.to_string(),
        "old_reserve1": old_reserve1.to_string(),
        "sqrt_price_x96": sqrt_price_x96.map(|v| v.to_string()),
        "liquidity": liquidity.map(|v| v.to_string()),
        "tick": tick,
        "best_route": {
            "merged_amounts": opportunity.best_route.as_ref().map(|r| r.merged_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
            // "merged_symbols": opportunity.best_route.as_ref().map(|r| r.merged_symbols.clone()),
            // "merged_pools": opportunity.best_route.as_ref().map(|r| r.merged_pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
            "profit": opportunity.best_route.as_ref().map(|r| r.profit.to_string()),
            "buy_path_hops": opportunity.best_route.as_ref().map(|r| r.buy_path.hops.clone()),
            "sell_path_hops": opportunity.best_route.as_ref().map(|r| r.sell_path.hops.clone()),
            "buy_amounts": opportunity.best_route.as_ref().map(|r| r.buy_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
            "sell_amounts": opportunity.best_route.as_ref().map(|r| r.sell_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
            "profit_percentage": opportunity.best_route.as_ref().map(|r| r.profit_percentage),
            "buy_path_pools": opportunity.best_route.as_ref().map(|r| r.buy_path.pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
            "sell_path_pools": opportunity.best_route.as_ref().map(|r| r.sell_path.pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
            "buy_symbols": opportunity.best_route.as_ref().map(|r| r.buy_symbols.clone()),
            "sell_symbols": opportunity.best_route.as_ref().map(|r| r.sell_symbols.clone()),
            "buy_pools": opportunity.best_route.as_ref().map(|r| r.buy_pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
            "sell_pools": opportunity.best_route.as_ref().map(|r| r.sell_pools.iter().map(|p| format!("0x{:x}", p)).collect::<Vec<_>>()),
            // "sell_test_amounts": opportunity.best_route.as_ref().map(|r| r.sell_test_amounts.iter().map(|a| a.to_string()).collect::<Vec<_>>()),

        }
    });

    // Add pool-wise data as a separate field
    // if let Some(best_route) = &opportunity.best_route {
    //     let mut pools_data = serde_json::Map::new();
    //     for pool_address in &best_route.merged_pools {
    //         let pool_key = format!("0x{:x}", pool_address);
    //         let mut pool_info = serde_json::Map::new();

    //         if let Some(state) = reserve_cache.get(pool_address) {
    //             pool_info.insert("reserve0".to_string(),
    //                 state.reserve0.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
    //             pool_info.insert("reserve1".to_string(),
    //                 state.reserve1.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
    //             pool_info.insert("sqrt_price_x96".to_string(),
    //                 state.sqrt_price_x96.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
    //             pool_info.insert("liquidity".to_string(),
    //                 state.liquidity.map(|v| serde_json::Value::String(v.to_string())).unwrap_or(serde_json::Value::Null));
    //             pool_info.insert("tick".to_string(),
    //                 state.tick.map(|v| serde_json::Value::Number(serde_json::Number::from(v))).unwrap_or(serde_json::Value::Null));
    //             pool_info.insert("fee".to_string(),
    //                 state.fee.map(|v| serde_json::Value::Number(serde_json::Number::from(v))).unwrap_or(serde_json::Value::Null));
    //             pool_info.insert("last_updated".to_string(),
    //                 serde_json::Value::Number(serde_json::Number::from(state.last_updated)));
    //             pool_info.insert("pool_type".to_string(),
    //                 serde_json::Value::String(format!("{:?}", state.pool_type)));
    //         } else {
    //             pool_info.insert("reserve0".to_string(), serde_json::Value::Null);
    //             pool_info.insert("reserve1".to_string(), serde_json::Value::Null);
    //             pool_info.insert("sqrt_price_x96".to_string(), serde_json::Value::Null);
    //             pool_info.insert("liquidity".to_string(), serde_json::Value::Null);
    //             pool_info.insert("tick".to_string(), serde_json::Value::Null);
    //             pool_info.insert("fee".to_string(), serde_json::Value::Null);
    //             pool_info.insert("last_updated".to_string(), serde_json::Value::Null);
    //             pool_info.insert("pool_type".to_string(), serde_json::Value::String("Unknown".to_string()));
    //         }

    //         pools_data.insert(pool_key, serde_json::Value::Object(pool_info));
    //     }

    //     log_entry.as_object_mut().unwrap().insert("merged_pools_data".to_string(), serde_json::Value::Object(pools_data));
    // }

    // Write to log file
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)
    {
        if let Err(e) = writeln!(
            file,
            "{}",
            serde_json::to_string_pretty(&log_entry).unwrap()
        ) {
            eprintln!("❌ [Price Tracker] Failed to write to log file: {}", e);
        }
    } else {
        eprintln!(
            "❌ [Price Tracker] Failed to open log file: {}",
            log_file_path
        );
    }

    // Also print summary to console
    // println!(
    //     "📝 [Price Tracker] Logged opportunity to: {} (latency: {} ms)",
    //     log_file_path, latency_ms
    // );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_logdata_string() {
        // Example stringified LogData (as bytes)
        let logdata_str = r#"LogData { topics: [0x8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925, 0x00000000000000000000000057f881845b20b943532f96758e94754fe7fb41e5, 0x0000000000000000000000349d363fa8ffdefe2332109280c5e66e48152c08], data: 0x0000000000000000000000000000000000000000000000003635c9adc5dea000 }"#;
        let logdata_bytes = logdata_str.as_bytes();
        let (topics, data_hex) = parse_logdata_string(logdata_bytes);
        println!("Extracted topics:");
        for (i, t) in topics.iter().enumerate() {
            println!("  topics[{}]: {}", i, t);
        }
        println!("Extracted data: {}", data_hex);
        // Optionally, add asserts for automated testing
        assert_eq!(
            topics[0],
            "0x8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925"
        );
        assert_eq!(
            topics[1],
            "0x00000000000000000000000057f881845b20b943532f96758e94754fe7fb41e5"
        );
        assert_eq!(
            topics[2],
            "0x0000000000000000000000349d363fa8ffdefe2332109280c5e66e48152c08"
        );
        assert_eq!(
            data_hex,
            "0x0000000000000000000000000000000000000000000000003635c9adc5dea000"
        );
    }

    #[test]
    fn test_print_simresult_logs() {
        // Simulate a SimResult with one SimLog containing stringified LogData
        let logdata_str = r#"LogData { topics: [0x8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925, 0x00000000000000000000000057f881845b20b943532f96758e94754fe7fb41e5, 0x0000000000000000000000349d363fa8ffdefe2332109280c5e66e48152c08], data: 0x0000000000000000000000000000000000000000000000003635c9adc5dea000 }"#;
        let sim_log = SimLog {
            address: hex::decode("7045e3f0456daad3176e1b51cbd94e86b44ca99d").unwrap(),
            topics: vec![],
            data: logdata_str.as_bytes().to_vec(),
        };
        let sim_result = SimResult {
            status: "success".to_string(),
            gas_used: 46333,
            output: None,
            logs: vec![sim_log],
        };
        print_simresult_logs(&sim_result);
    }

    // Demo test for MyTracer (does not run a real EVM, just shows struct usage)
    //     #[test]
    //     fn test_print_call_trace() {
    //         let mut root = CallTraceNode {
    //             call_type: "CALL".to_string(),
    //             from: Address::ZERO,
    //             to: Address::ZERO,
    //             value: B256::from(1),
    //             input: Bytes::from(vec![1,2,3]),
    //             output: Some(Bytes::from(vec![4,5,6])),
    //             depth: 0,
    //             children: vec![],
    //             logs: vec![TraceLog {
    //                 address: Address::ZERO,
    //                 topics: vec![B256::ZERO],
    //                 data: Bytes::from(vec![7,8,9]),
    //             }],
    //         };
    //         let child = CallTraceNode {
    //             call_type: "CALL".to_string(),
    //             from: Address::ZERO,
    //             to: Address::ZERO,
    //             value: U256::from(2),
    //             input: Bytes::from(vec![10,11]),
    //             output: None,
    //             depth: 1,
    //             children: vec![],
    //             logs: vec![],
    //         };
    //         root.children.push(child);
    //         print_call_trace(&root, 0);
    //     }

    //     #[test]
    //     fn test_swap_v3_decode_manual() {
    //         use ethers::abi::{ParamType, decode, Token};
    //         use num_bigint::BigInt;
    //         let data_hex = "0xfffffffffffffffffffffffffffffffffffffffffffffff69dbe8ebd26a0f47500000000000000000000000000000000000000000000001b1ae4d6e2ef5000000000000000000000000000000000000000000001b167c1883204379306f55c2f000000000000000000000000000000000000000000001adbe22ae2898083e1360000000000000000000000000000000000000000000000000000000000002922";
    //         let data_bytes = hex::decode(data_hex.trim_start_matches("0x")).unwrap();
    //         println!("data_bytes.len(): {}", data_bytes.len());
    //         let param_types = vec![
    //             ParamType::Int(256),
    //             ParamType::Int(256),
    //             ParamType::Uint(160),
    //             ParamType::Uint(128),
    //             ParamType::Int(24),
    //         ];
    //         let tokens = decode(&param_types, &data_bytes).unwrap();
    //         // Print amount0 as signed
    //         let amount0 = match &tokens[0] {
    //             Token::Int(i) => {
    //                 let mut buf = [0u8; 32];
    //                 i.to_big_endian(&mut buf);
    //                 BigInt::from_signed_bytes_be(&buf)
    //             },
    //             _ => panic!("not int"),
    //         };
    //         let amount1 = match &tokens[1] {
    //             Token::Int(i) => {
    //                 let mut buf = [0u8; 32];
    //                 i.to_big_endian(&mut buf);
    //                 BigInt::from_signed_bytes_be(&buf)
    //             },
    //             _ => panic!("not int"),
    //         };
    //         println!("amount0: {}", amount0);
    //         println!("amount1: {}", amount1);
    //         println!("sqrtPriceX96: {}", tokens[2].clone().into_uint().unwrap());
    //         println!("liquidity: {}", tokens[3].clone().into_uint().unwrap());
    //         let tick = match &tokens[4] {
    //             Token::Int(i) => {
    //                 let mut buf = [0u8; 32];
    //                 i.to_big_endian(&mut buf);
    //                 BigInt::from_signed_bytes_be(&buf)
    //             },
    //             _ => panic!("not int"),
    //         };
    //         println!("tick: {}", tick);
    //     }
}

/// Walks the call trace, updates the reserve cache for any Sync/Swap events, and checks for arbitrage opportunities.
pub async fn process_simulation_events_and_arbitrage(
    trace: &CallTraceNode,
    reserve_cache: &Arc<ReserveCache>,
    token_index: &Arc<TokenIndexMap>,
    precomputed_route_cache: &Arc<DashMap<u32, Vec<RoutePath>>>,
    opportunity_tx: &mpsc::Sender<ArbitrageOpportunity>,
    token_tax_map: &Arc<TokenTaxMap>,
    config: &crate::config::Config,
) {
    // Helper to recursively walk the call trace
    fn walk_trace<'a>(node: &'a CallTraceNode, out: &mut Vec<(&'a TraceLog, &'a CallTraceNode)>) {
        for log in &node.logs {
            out.push((log, node));
        }
        for child in &node.children {
            walk_trace(child, out);
        }
    }
    let mut logs_with_nodes = Vec::new();
    walk_trace(trace, &mut logs_with_nodes);

    for (log, node) in logs_with_nodes {
        let (topics, data_hex) = crate::revm_sim::parse_logdata_string(&log.data);
        // Sync V2 event
        let sync_topic = format!(
            "0x{:x}",
            alloy_primitives::keccak256("Sync(uint112,uint112)")
        );
        let swap_v2_topic = format!(
            "0x{:x}",
            alloy_primitives::keccak256(
                "Swap(address,address,uint256,uint256,uint256,uint256,address)"
            )
        );
        let swap_v3_topic = format!(
            "0x{:x}",
            alloy_primitives::keccak256(
                "Swap(address,address,int256,int256,uint160,uint128,int24)"
            )
        );
        let pancake_v3_topic = format!(
            "0x{:x}",
            alloy_primitives::keccak256(
                "Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)"
            )
        );
        if let Some(topic0) = topics.get(0) {
            // --- V2 Sync ---
            if topic0 == &sync_topic && data_hex.len() >= 2 + 64 {
                if let Ok(data_bytes) = hex::decode(data_hex.trim_start_matches("0x")) {
                    if data_bytes.len() >= 64 {
                        let new_reserve0 = eU256::from_big_endian(&data_bytes[0..32]);
                        let new_reserve1 = eU256::from_big_endian(&data_bytes[32..64]);
                        let pool = H160::from_slice(log.address.0.as_slice());
                        // Update cache
                        if let Some(mut state) = reserve_cache.get_mut(&pool) {
                            state.reserve0 = Some(new_reserve0);
                            state.reserve1 = Some(new_reserve1);
                            state.last_updated = chrono::Utc::now().timestamp() as u64;
                        }
                        // Arbitrage check (like price_tracker)
                        let decoded_swap = DecodedSwap {
                            tx_hash: H160::zero(), // Mempool sim, so no real tx hash
                            pool_address: pool,
                            token_x: H160::zero(),         // Not used for now
                            token_x_amount: eU256::zero(), // Not used for now
                            block_number: 0,
                            timestamp: chrono::Utc::now().timestamp() as u64,
                        };
                        if let Some((opportunity, _latency)) =
                            crate::price_tracker::find_arbitrage_opportunity_from_price_tracker(
                                &decoded_swap,
                                reserve_cache,
                                token_index,
                                precomputed_route_cache,
                                token_tax_map,
                                config,
                            )
                            .await
                        {
                            let _ = opportunity_tx.send(opportunity).await;
                        }
                    }
                }
            }
            // --- V3 Swap ---
            if (topic0 == &swap_v3_topic || topic0 == &pancake_v3_topic)
                && data_hex.len() >= 2 + 160
            {
                if let Ok(data_bytes) = hex::decode(data_hex.trim_start_matches("0x")) {
                    // Uniswap V3: 160 bytes, Pancake V3: 224 bytes
                    let (sqrt_price_x96, liquidity, tick) = if data_bytes.len() == 160 {
                        // Uniswap V3
                        let sqrt_price_x96 = eU256::from_big_endian(&data_bytes[64..84]);
                        let liquidity = eU256::from_big_endian(&data_bytes[84..100]);
                        let tick = {
                            let mut buf = [0u8; 32];
                            buf[8..32].copy_from_slice(&data_bytes[100..124]);
                            eU256::from_big_endian(&buf)
                        };
                        (sqrt_price_x96, liquidity, tick)
                    } else if data_bytes.len() == 224 {
                        // Pancake V3
                        let sqrt_price_x96 = eU256::from_big_endian(&data_bytes[64..84]);
                        let liquidity = eU256::from_big_endian(&data_bytes[84..100]);
                        let tick = {
                            let mut buf = [0u8; 32];
                            buf[8..32].copy_from_slice(&data_bytes[100..124]);
                            eU256::from_big_endian(&buf)
                        };
                        (sqrt_price_x96, liquidity, tick)
                    } else {
                        (eU256::zero(), eU256::zero(), eU256::zero())
                    };
                    let pool = H160::from_slice(log.address.0.as_slice());
                    if let Some(mut state) = reserve_cache.get_mut(&pool) {
                        state.sqrt_price_x96 = Some(sqrt_price_x96);
                        state.liquidity = Some(liquidity);
                        state.tick = Some(tick.as_u32() as i32);
                        state.last_updated = chrono::Utc::now().timestamp() as u64;
                    }
                    // Arbitrage check (like price_tracker)
                    let decoded_swap = DecodedSwap {
                        tx_hash: H160::zero(),
                        pool_address: pool,
                        token_x: H160::zero(),
                        token_x_amount: eU256::zero(),
                        block_number: 0,
                        timestamp: chrono::Utc::now().timestamp() as u64,
                    };
                    if let Some((opportunity, _latency)) =
                        crate::price_tracker::find_arbitrage_opportunity_from_price_tracker(
                            &decoded_swap,
                            reserve_cache,
                            token_index,
                            precomputed_route_cache,
                            token_tax_map,
                            config,
                        )
                        .await
                    {
                        let _ = opportunity_tx.send(opportunity).await;
                    }
                }
            }
        }
    }
}

/// Walks the call trace tree and returns true if any log emits a SwapV2, SwapV3, or SyncV2 event
fn trace_has_dex_event(node: &CallTraceNode) -> bool {
    // DEX event topics
    let swap_v2 = format!(
        "0x{:x}",
        keccak256("Swap(address,address,uint256,uint256,uint256,uint256,address)")
    );
    let swap_v3 = format!(
        "0x{:x}",
        keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)")
    );
    let sync_v2 = format!("0x{:x}", keccak256("Sync(uint112,uint112)"));
    for log in &node.logs {
        let (topics, _) = parse_logdata_string2(&log.data);
        if let Some(topic0) = topics.get(0) {
            if topic0 == &swap_v2 || topic0 == &swap_v3 || topic0 == &sync_v2 {
                return true;
            }
        }
    }
    for child in &node.children {
        if trace_has_dex_event(child) {
            return true;
        }
    }
    false
}

/// Shallow trace: simulate call trace and check for DEX event logs
pub async fn shallow_trace_for_pool(
    tx: &Transaction,
    _known_router_cache: Arc<Mutex<HashSet<String>>>,
    provider: Arc<DynProvider>,
) -> Option<String> {
    use crate::utils::ethers_tx_to_revm_txenv;
    let tx_env = ethers_tx_to_revm_txenv(tx);
    let sim = RevmSimulator::new();
    // Use simulate_with_trace for call trace (no state commit)
    let trace_opt = sim
        .simulate_with_forked_state(tx_env, provider)
        .await
        .ok()
        .flatten();
    if let Some(trace) = trace_opt {
        if trace_has_dex_event(&trace) {
            // Add the root contract address (tx.to) as a new known_router
            return tx.to.map(|a| format!("0x{:x}", a));
        }
    }
    None
}
