//! Coherent EVM construction from one resolved network profile.

use crate::{
    executors::{Executor, ExecutorBuilder},
    inspectors::InspectorStackBuilder,
};
use alloy_evm::{Evm, EvmFactory};
use alloy_primitives::{Address, BlockNumber};
use foundry_config::Config;
use foundry_evm_core::{
    FoundryBlock, FoundryTransaction,
    backend::{Backend, DatabaseExt},
    evm::{BlockEnvFor, EvmEnvFor, FoundryEvmNetwork, SpecFor, TxEnvFor},
    opts::EvmOpts,
};
use foundry_evm_networks::{
    NetworkExecutionContext, PrecompileCompositionError, ResolvedNetworkProfile,
};
use foundry_evm_traces::{
    CallTrace, CallTraceDecoder, CallTraceDecoderBuilder, DecodedCallTrace, TraceMode,
};
use revm::{context::Block, database::EmptyDB};
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};
use thiserror::Error;

/// Errors returned before an EVM construction can execute.
#[derive(Debug, Error)]
pub enum EvmConstructionError {
    /// The resolved profile does not match the selected generic EVM family.
    #[error(
        "network profile `{profile}` requires the `{required}` EVM family, but `{selected}` was selected"
    )]
    ProfileFamilyMismatch {
        /// Resolved profile name.
        profile: &'static str,
        /// Family required by the profile.
        required: &'static str,
        /// Family selected by the generic network binding.
        selected: &'static str,
    },
    /// The fork material carries a different profile from the command-owned profile.
    #[error("fork profile `{fork}` does not match prepared profile `{prepared}`")]
    ForkProfileMismatch {
        /// Profile carried by the fork request.
        fork: &'static str,
        /// Profile owned by the preparation.
        prepared: &'static str,
    },
    /// Environment or transaction preparation failed.
    #[error("failed to prepare EVM environment: {0}")]
    Environment(String),
    /// Reusable backend state preparation failed.
    #[error("failed to prepare reusable EVM state: {0}")]
    Backend(String),
    /// Network precompiles could not be composed for the current environment.
    #[error(transparent)]
    PrecompileComposition(#[from] PrecompileCompositionError),
}

/// Entry point for preparing coherent EVM construction state.
#[derive(Clone, Copy, Debug, Default)]
pub struct EvmConstruction;

impl EvmConstruction {
    /// Prepares environment, optional fork material, and reusable backend state.
    pub async fn prepare<FEN: FoundryEvmNetwork>(
        evm_opts: &EvmOpts,
        config: &Config,
        network_profile: ResolvedNetworkProfile,
    ) -> Result<PreparedEvm<FEN>, EvmConstructionError>
    where
        SpecFor<FEN>: Into<revm::primitives::hardfork::SpecId> + Default + Copy,
        BlockEnvFor<FEN>: FoundryBlock + Default,
        TxEnvFor<FEN>: FoundryTransaction + Default,
    {
        validate_family::<FEN>(network_profile)?;
        let (evm_env, tx_env, fork_block_number) = evm_opts
            .env_with_network_profile::<SpecFor<FEN>, BlockEnvFor<FEN>, TxEnvFor<FEN>>(
                network_profile,
            )
            .await
            .map_err(|error| EvmConstructionError::Environment(error.to_string()))?;
        let fork = evm_opts.get_fork_with_network_profile(
            config,
            evm_env.cfg_env.chain_id,
            fork_block_number,
            network_profile,
        );
        if let Some(fork) = &fork
            && fork.network_profile != network_profile
        {
            return Err(EvmConstructionError::ForkProfileMismatch {
                fork: fork.network_profile.name(),
                prepared: network_profile.name(),
            });
        }

        let network_context = execution_context::<FEN>(&evm_env);
        let backend = Backend::spawn_with_network_profile(fork, network_profile, network_context)
            .map_err(|error| EvmConstructionError::Backend(error.to_string()))?;
        if backend.network_profile() != network_profile {
            return Err(EvmConstructionError::ForkProfileMismatch {
                fork: backend.network_profile().name(),
                prepared: network_profile.name(),
            });
        }

        Ok(PreparedEvm {
            backend,
            evm_env,
            tx_env,
            fork_block_number,
            network_profile,
            network_context,
        })
    }
}

/// Opaque environment and reusable state prepared for one construction.
pub struct PreparedEvm<FEN: FoundryEvmNetwork> {
    backend: Backend<FEN>,
    evm_env: EvmEnvFor<FEN>,
    tx_env: TxEnvFor<FEN>,
    fork_block_number: Option<BlockNumber>,
    network_profile: ResolvedNetworkProfile,
    network_context: NetworkExecutionContext,
}

impl<FEN: FoundryEvmNetwork> PreparedEvm<FEN> {
    /// Returns the chain ID of the prepared environment.
    pub const fn chain_id(&self) -> u64 {
        self.network_context.chain_id
    }

    /// Returns the timestamp from which construction derives its activation snapshot.
    pub const fn timestamp(&self) -> u64 {
        self.network_context.timestamp
    }

    /// Returns the pinned fork block, if this is a fork preparation.
    pub const fn fork_block_number(&self) -> Option<BlockNumber> {
        self.fork_block_number
    }

    /// Constructs an executor and decoder bound to one environment snapshot.
    pub fn construct(
        self,
        config: ExecutorConfig<FEN>,
    ) -> Result<ConstructedEvm<FEN>, EvmConstructionError> {
        let Self { backend, evm_env, tx_env, network_profile, network_context, .. } = self;
        validate_precompile_composition::<FEN>(network_profile, network_context, &evm_env)?;

        let ExecutorConfig { inspectors, gas_limit, spec, legacy_assertions, .. } = config;
        let mut builder = ExecutorBuilder::default()
            .inspectors(|_| inspectors.network_profile(network_profile))
            .spec_id_opt(spec)
            .legacy_assertions(legacy_assertions);
        if let Some(gas_limit) = gas_limit {
            builder = builder.gas_limit(gas_limit);
        }
        let executor = builder.build(evm_env, tx_env, backend);
        let decoder = CallTraceDecoderBuilder::new()
            .with_chain_id(Some(network_context.chain_id))
            .with_network_profile(network_profile, network_context)
            .build();

        Ok(ConstructedEvm {
            executor,
            decoder,
            chain_id: network_context.chain_id,
            timestamp: network_context.timestamp,
        })
    }
}

/// Consumer-specific executor behavior without network selection fields.
#[derive(Debug)]
#[must_use = "construction config does nothing unless passed to `PreparedEvm::construct`"]
pub struct ExecutorConfig<FEN: FoundryEvmNetwork> {
    inspectors: InspectorStackBuilder<BlockEnvFor<FEN>>,
    gas_limit: Option<u64>,
    spec: Option<SpecFor<FEN>>,
    legacy_assertions: bool,
    marker: PhantomData<FEN>,
}

impl<FEN: FoundryEvmNetwork> Default for ExecutorConfig<FEN> {
    fn default() -> Self {
        Self {
            inspectors: InspectorStackBuilder::default(),
            gas_limit: None,
            spec: None,
            legacy_assertions: false,
            marker: PhantomData,
        }
    }
}

impl<FEN: FoundryEvmNetwork> ExecutorConfig<FEN> {
    /// Enables the requested trace collection mode.
    pub fn trace_mode(mut self, trace_mode: TraceMode) -> Self {
        self.inspectors = self.inspectors.trace_mode(trace_mode);
        self
    }

    /// Enables or disables isolated top-level calls.
    pub fn enable_isolation(mut self, enable: bool) -> Self {
        self.inspectors = self.inspectors.enable_isolation(enable);
        self
    }

    /// Sets the CREATE2 deployer used by inspector behavior.
    pub fn create2_deployer(mut self, address: Address) -> Self {
        self.inspectors = self.inspectors.create2_deployer(address);
        self
    }

    /// Overrides the executor gas limit.
    pub const fn gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = Some(gas_limit);
        self
    }

    /// Overrides the EVM spec used by the executor.
    pub const fn spec_id(mut self, spec: SpecFor<FEN>) -> Self {
        self.spec = Some(spec);
        self
    }

    /// Enables legacy DSTest assertion probing.
    pub const fn legacy_assertions(mut self, enabled: bool) -> Self {
        self.legacy_assertions = enabled;
        self
    }
}

/// Opaque executor and trace decoder produced from one construction snapshot.
pub struct ConstructedEvm<FEN: FoundryEvmNetwork> {
    executor: Executor<FEN>,
    decoder: CallTraceDecoder,
    chain_id: u64,
    timestamp: u64,
}

impl<FEN: FoundryEvmNetwork> ConstructedEvm<FEN> {
    /// Returns the constructed chain ID.
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Returns the timestamp fixed for this construction.
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Decodes one call using the decoder bound to this construction.
    pub async fn decode_function(&self, trace: &CallTrace) -> DecodedCallTrace {
        self.decoder.decode_function(trace).await
    }
}

impl<FEN: FoundryEvmNetwork> Deref for ConstructedEvm<FEN> {
    type Target = Executor<FEN>;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

impl<FEN: FoundryEvmNetwork> DerefMut for ConstructedEvm<FEN> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.executor
    }
}

fn execution_context<FEN: FoundryEvmNetwork>(evm_env: &EvmEnvFor<FEN>) -> NetworkExecutionContext {
    NetworkExecutionContext::new(
        evm_env.cfg_env.chain_id,
        evm_env.block_env.timestamp().saturating_to(),
    )
}

fn validate_family<FEN: FoundryEvmNetwork>(
    network_profile: ResolvedNetworkProfile,
) -> Result<(), EvmConstructionError> {
    let required = network_profile.evm_family();
    let selected = FEN::EVM_FAMILY;
    if required != selected {
        return Err(EvmConstructionError::ProfileFamilyMismatch {
            profile: network_profile.name(),
            required: required.name(),
            selected: selected.name(),
        });
    }
    Ok(())
}

fn validate_precompile_composition<FEN: FoundryEvmNetwork>(
    network_profile: ResolvedNetworkProfile,
    network_context: NetworkExecutionContext,
    evm_env: &EvmEnvFor<FEN>,
) -> Result<(), PrecompileCompositionError> {
    let mut evm = FEN::EvmFactory::default().create_evm(EmptyDB::default(), evm_env.clone());
    network_profile.inject_precompiles(evm.precompiles_mut(), network_context)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "hashkey")]
    use alloy_primitives::address;
    #[cfg(feature = "hashkey")]
    use foundry_config::GasLimit;
    use foundry_evm_core::evm::EthEvmNetwork;
    #[cfg(feature = "hashkey")]
    use foundry_evm_core::evm::OpEvmNetwork;
    use foundry_evm_networks::NetworkConfigs;

    #[cfg(feature = "hashkey")]
    const B20_FACTORY: Address = address!("B20F000000000000000000000000000000000000");

    #[tokio::test]
    async fn rejects_profile_family_mismatch_before_preparation() {
        let error = match EvmConstruction::prepare::<EthEvmNetwork>(
            &EvmOpts::default(),
            &Config::default(),
            NetworkConfigs::with_optimism().resolve(),
        )
        .await
        {
            Ok(_) => panic!("mismatched family must fail"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            EvmConstructionError::ProfileFamilyMismatch {
                required: "optimism",
                selected: "ethereum",
                ..
            }
        ));
    }

    #[cfg(feature = "hashkey")]
    #[tokio::test(flavor = "multi_thread")]
    async fn construction_binds_execution_and_decoder_to_one_snapshot() {
        let mut evm_opts = EvmOpts::default();
        evm_opts.env.chain_id = Some(177);
        evm_opts.env.gas_limit = GasLimit(30_000_000);
        let profile = NetworkConfigs::with_hashkey().resolve();

        let normal =
            EvmConstruction::prepare::<OpEvmNetwork>(&evm_opts, &Config::default(), profile)
                .await
                .unwrap()
                .construct(ExecutorConfig::default())
                .unwrap();
        let traced =
            EvmConstruction::prepare::<OpEvmNetwork>(&evm_opts, &Config::default(), profile)
                .await
                .unwrap()
                .construct(ExecutorConfig::default().trace_mode(TraceMode::Call))
                .unwrap();
        let isolated =
            EvmConstruction::prepare::<OpEvmNetwork>(&evm_opts, &Config::default(), profile)
                .await
                .unwrap()
                .construct(ExecutorConfig::default().enable_isolation(true))
                .unwrap();

        let trace = CallTrace { address: B20_FACTORY, ..Default::default() };
        let normal_decoded = normal.decode_function(&trace).await;
        let traced_decoded = traced.decode_function(&trace).await;
        let isolated_decoded = isolated.decode_function(&trace).await;
        assert_eq!(normal_decoded.label.as_deref(), Some("B20Factory"));
        assert_eq!(normal_decoded, traced_decoded);
        assert_eq!(normal_decoded, isolated_decoded);
        assert_eq!((normal.chain_id(), normal.timestamp()), (177, 0));
    }
}
