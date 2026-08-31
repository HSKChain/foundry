//! ChiselRunner
//!
//! This module contains the `ChiselRunner` struct, which assists with deploying
//! and calling the REPL contract on a in-memory REVM instance.

use alloy_primitives::{Address, Bytes, Log, U256, map::AddressHashMap};
use eyre::Result;
use foundry_evm::{
    construction::{ConstructedEvm, DecoderConfig, ReusableEvmState},
    core::evm::{EthEvmNetwork, FoundryEvmNetwork},
    executors::{DeployResult, RawCallResult},
    traces::{CallTraceDecoder, TraceKind, Traces},
};

/// The function selector of the REPL contract's entrypoint, the `run()` function.
static RUN_SELECTOR: [u8; 4] = [0xc0, 0x40, 0x62, 0x26];

/// The Chisel Runner
///
/// Based off of foundry's forge cli runner for scripting.
/// See: [runner](cli::cmd::forge::script::runner.rs)
pub struct ChiselRunner<FEN: FoundryEvmNetwork = EthEvmNetwork> {
    /// The Executor
    pub executor: ConstructedEvm<FEN>,
    /// An initial balance
    pub initial_balance: U256,
    /// The sender
    pub sender: Address,
    /// Input calldata appended to `RUN_SELECTOR`
    pub input: Option<Vec<u8>>,
}

/// Represents the result of a Chisel REPL run
#[derive(Debug, Default)]
pub struct ChiselResult {
    /// Was the run a success?
    pub success: bool,
    /// Transaction logs
    pub logs: Vec<Log>,
    /// Call traces
    pub traces: Traces,
    /// Amount of gas used in the transaction
    pub gas_used: u64,
    /// Map of addresses to their labels
    pub labeled_addresses: AddressHashMap<String>,
    /// Return data
    pub returned: Bytes,
    /// Called address
    pub address: Address,
    /// Trace decoder bound to the execution construction snapshot.
    pub decoder: CallTraceDecoder,
    /// EVM State at the final instruction of the `run()` function
    pub state: Option<(Vec<U256>, Vec<u8>)>,
}

/// ChiselRunner implementation
impl<FEN: FoundryEvmNetwork> ChiselRunner<FEN> {
    /// Create a new [ChiselRunner]
    ///
    /// ### Takes
    ///
    /// A [ConstructedEvm], the initial balance of the sender, and the sender's [Address].
    ///
    /// ### Returns
    ///
    /// A new [ChiselRunner]
    pub const fn new(
        executor: ConstructedEvm<FEN>,
        initial_balance: U256,
        sender: Address,
        input: Option<Vec<u8>>,
    ) -> Self {
        Self { executor, initial_balance, sender, input }
    }

    /// Returns the runner's current reusable backend state.
    pub fn reusable_state(&self) -> ReusableEvmState<FEN> {
        self.executor.reusable_state()
    }

    /// Run a contract as a REPL session
    pub fn run(&mut self, bytecode: Bytes) -> Result<ChiselResult> {
        // Set the sender's balance to [U256::MAX] for deployment of the REPL contract.
        self.executor.set_balance(self.sender, U256::MAX)?;

        // Deploy an instance of the REPL contract
        // We don't care about deployment traces / logs here
        let DeployResult { address, .. } = self
            .executor
            .deploy(self.sender, bytecode, U256::ZERO, None)
            .map_err(|err| eyre::eyre!("Failed to deploy REPL contract:\n{}", err))?;

        // Reset the sender's balance to the initial balance for calls.
        self.executor.set_balance(self.sender, self.initial_balance)?;

        // Append the input to the `RUN_SELECTOR` to form the calldata
        let mut calldata = RUN_SELECTOR.to_vec();
        if let Some(mut input) = self.input.clone() {
            calldata.append(&mut input);
        }

        let res = self.executor.transact_raw(self.sender, address, calldata.into(), U256::ZERO)?;

        let RawCallResult {
            result, reverted, logs, traces, labels, chisel_state, gas_used, ..
        } = res;
        let decoder = self.executor.trace_decoder(DecoderConfig::default().labels(labels.clone()));

        Ok(ChiselResult {
            returned: result,
            success: !reverted,
            gas_used,
            logs,
            traces: traces.map(|traces| vec![(TraceKind::Execution, traces)]).unwrap_or_default(),
            labeled_addresses: labels,
            address,
            decoder,
            state: chisel_state,
        })
    }
}
