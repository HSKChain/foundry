use crate::{
    construction::{ConstructedEvm, DecoderConfig, EvmConstruction, ExecutorConfig, PreparedEvm},
    executors::Executor,
};
use alloy_primitives::{Address, U256, map::HashMap};
use alloy_rpc_types::state::StateOverride;
use eyre::Context;
use foundry_compilers::artifacts::EvmVersion;
use foundry_config::{Chain, Config, evm_spec_id};
use foundry_evm_core::{
    FoundryBlock, FoundryTransaction,
    evm::{BlockEnvFor, FoundryEvmNetwork, SpecFor, TxEnvFor},
    opts::resolution::ResolvedEvmOpts,
};
use foundry_evm_hardforks::TempoHardfork;
use foundry_evm_traces::{CallTraceDecoder, TraceMode};
use revm::state::Bytecode;
use std::ops::{Deref, DerefMut};

/// A default executor with tracing enabled
pub struct TracingExecutor<FEN: FoundryEvmNetwork> {
    constructed: ConstructedEvm<FEN>,
}

impl<FEN: FoundryEvmNetwork> TracingExecutor<FEN> {
    pub fn new(
        prepared: PreparedEvm<FEN>,
        version: Option<EvmVersion>,
        trace_mode: TraceMode,
        create2_deployer: Address,
        state_overrides: Option<StateOverride>,
    ) -> eyre::Result<Self> {
        // configures a bare version of the evm executor: no cheatcode and log_collector inspector
        // is enabled, tracing will be enabled only for the targeted transaction
        let mut constructed = prepared.construct(
            ExecutorConfig::default()
                .trace_mode(trace_mode)
                .create2_deployer(create2_deployer)
                .spec_id_opt(version.map(evm_spec_id::<SpecFor<FEN>>)),
        )?;

        // Apply the state overrides.
        if let Some(state_overrides) = state_overrides {
            for (address, overrides) in state_overrides {
                if let Some(balance) = overrides.balance {
                    constructed.set_balance(address, balance)?;
                }
                if let Some(nonce) = overrides.nonce {
                    constructed.set_nonce(address, nonce)?;
                }
                if let Some(code) = overrides.code {
                    let bytecode = Bytecode::new_raw_checked(code)
                        .wrap_err("invalid bytecode in state override")?;
                    constructed.set_code(address, bytecode)?;
                }
                if let Some(state) = overrides.state {
                    let state: HashMap<U256, U256> = state
                        .into_iter()
                        .map(|(slot, value)| (slot.into(), value.into()))
                        .collect();
                    constructed.set_storage(address, state)?;
                }
                if let Some(state_diff) = overrides.state_diff {
                    for (slot, value) in state_diff {
                        constructed.set_storage_slot(address, slot.into(), value.into())?;
                    }
                }
            }
        }

        Ok(Self { constructed })
    }

    /// Returns the spec id of the executor
    pub fn spec_id(&self) -> SpecFor<FEN> {
        self.constructed.spec_id()
    }

    /// Returns the chain ID bound to this executor's construction snapshot.
    pub const fn chain_id(&self) -> u64 {
        self.constructed.chain_id()
    }

    /// Returns the decoder bound to the same snapshot as this executor.
    pub const fn decoder(&self) -> &CallTraceDecoder {
        self.constructed.decoder()
    }

    /// Builds a consumer decoder bound to the same snapshot as this executor.
    pub fn trace_decoder(&self, config: DecoderConfig) -> CallTraceDecoder {
        self.constructed.trace_decoder(config)
    }

    /// uses the fork block number from the config
    pub async fn prepare(
        config: &mut Config,
        resolved: ResolvedEvmOpts,
    ) -> eyre::Result<(PreparedEvm<FEN>, Chain)>
    where
        SpecFor<FEN>: Into<revm::primitives::hardfork::SpecId> + Default + Copy,
        BlockEnvFor<FEN>: FoundryBlock + Default,
        TxEnvFor<FEN>: FoundryTransaction + Default,
    {
        let resolved = resolved
            .with_fork_url(config.get_rpc_url_or_localhost_http()?.into_owned())
            .with_fork_block_number(config.fork_block_number);
        let network_profile = resolved.network_profile();

        let prepared = EvmConstruction::prepare::<FEN>(&resolved, config).await?;
        config
            .labels
            .extend(network_profile.precompile_labels(Some(config.evm_spec_id::<TempoHardfork>())));

        let chain = Chain::from_id(prepared.chain_id());
        Ok((prepared, chain))
    }
}

impl<FEN: FoundryEvmNetwork> Deref for TracingExecutor<FEN> {
    type Target = Executor<FEN>;

    fn deref(&self) -> &Self::Target {
        &self.constructed
    }
}

impl<FEN: FoundryEvmNetwork> DerefMut for TracingExecutor<FEN> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.constructed
    }
}

#[cfg(all(test, feature = "hashkey"))]
mod tests {
    use super::*;
    use alloy_primitives::address;
    use foundry_config::GasLimit;
    use foundry_evm_core::{
        evm::OpEvmNetwork,
        opts::{
            EvmOpts,
            resolution::{CommandProfileResolution, NetworkIntent},
        },
    };
    use foundry_evm_networks::NetworkConfigs;
    use foundry_evm_traces::CallTrace;

    #[tokio::test(flavor = "multi_thread")]
    async fn tracing_executor_uses_prepared_decoder_snapshot() {
        let mut evm_opts = EvmOpts::default();
        evm_opts.env.chain_id = Some(177);
        evm_opts.env.gas_limit = GasLimit(30_000_000);
        evm_opts.networks = NetworkConfigs::with_hashkey();
        let resolved = CommandProfileResolution::new()
            .resolve_evm_opts(evm_opts, NetworkIntent::new())
            .unwrap();
        let prepared =
            EvmConstruction::prepare::<OpEvmNetwork>(&resolved, &Config::default()).await.unwrap();

        let executor =
            TracingExecutor::new(prepared, None, TraceMode::Call, Address::ZERO, None).unwrap();
        let trace = CallTrace {
            address: address!("B20F000000000000000000000000000000000000"),
            ..Default::default()
        };

        assert_eq!(
            executor.constructed.decode_function(&trace).await.label.as_deref(),
            Some("B20Factory")
        );
    }
}
