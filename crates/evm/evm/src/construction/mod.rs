//! Coherent EVM construction from one resolved network profile.

use crate::{
    executors::{Executor, ExecutorBuilder},
    inspectors::{CheatsConfig, InspectorStackBuilder},
};
use alloy_evm::{Evm, EvmFactory};
use alloy_primitives::{Address, BlockNumber, map::AddressHashMap};
use foundry_cheatcodes::Wallets;
use foundry_common::{ContractsByArtifact, compile::Analysis};
use foundry_config::Config;
use foundry_evm_core::{
    FoundryBlock, FoundryTransaction,
    backend::{Backend, DatabaseExt, construction as backend_construction},
    evm::{BlockEnvFor, EvmEnvFor, FoundryEvmNetwork, SpecFor, TxEnvFor},
    opts::{construction as opts_construction, resolution::ResolvedEvmOpts},
};
use foundry_evm_networks::{
    NetworkExecutionContext, PrecompileCompositionError, ResolvedNetworkProfile,
};
use foundry_evm_traces::{
    CallTrace, CallTraceDecoder, CallTraceDecoderBuilder, DebugTraceIdentifier, DecodedCallTrace,
    TraceMode, bind_network_snapshot, identifier::SignaturesIdentifier,
};
use revm::{context::Block, database::EmptyDB};
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::Arc,
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
        resolved: &ResolvedEvmOpts,
        config: &Config,
    ) -> Result<PreparedEvm<FEN>, EvmConstructionError>
    where
        SpecFor<FEN>: Into<revm::primitives::hardfork::SpecId> + Default + Copy,
        BlockEnvFor<FEN>: FoundryBlock + Default,
        TxEnvFor<FEN>: FoundryTransaction + Default,
    {
        let evm_opts = resolved.evm_opts();
        let network_profile = resolved.network_profile();
        validate_family::<FEN>(network_profile)?;
        let prepared = opts_construction::prepare::<SpecFor<FEN>, BlockEnvFor<FEN>, TxEnvFor<FEN>>(
            evm_opts,
            Some(config),
            network_profile,
        )
        .await
        .map_err(|error| EvmConstructionError::Environment(error.to_string()))?;
        let opts_construction::PreparedEvmOpts { evm_env, tx_env, fork_block_number, fork } =
            prepared;
        if let Some(fork) = &fork
            && fork.network_profile != network_profile
        {
            return Err(EvmConstructionError::ForkProfileMismatch {
                fork: fork.network_profile.name(),
                prepared: network_profile.name(),
            });
        }

        let network_context = execution_context::<FEN>(&evm_env);
        let is_fork = fork.is_some();
        let backend = backend_construction::spawn(fork, network_profile)
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
            is_fork,
        })
    }

    /// Prepares a fresh environment snapshot over reusable opaque backend state.
    pub async fn prepare_with_state<FEN: FoundryEvmNetwork>(
        resolved: &ResolvedEvmOpts,
        state: &ReusableEvmState<FEN>,
    ) -> Result<PreparedEvm<FEN>, EvmConstructionError>
    where
        SpecFor<FEN>: Into<revm::primitives::hardfork::SpecId> + Default + Copy,
        BlockEnvFor<FEN>: FoundryBlock + Default,
        TxEnvFor<FEN>: FoundryTransaction + Default,
    {
        let network_profile = resolved.network_profile();
        if network_profile != state.network_profile {
            return Err(EvmConstructionError::ForkProfileMismatch {
                fork: state.network_profile.name(),
                prepared: network_profile.name(),
            });
        }
        validate_family::<FEN>(network_profile)?;
        let prepared = opts_construction::prepare::<SpecFor<FEN>, BlockEnvFor<FEN>, TxEnvFor<FEN>>(
            resolved.evm_opts(),
            None,
            network_profile,
        )
        .await
        .map_err(|error| EvmConstructionError::Environment(error.to_string()))?;
        let opts_construction::PreparedEvmOpts { evm_env, tx_env, fork_block_number, .. } =
            prepared;
        if state.backend.network_profile() != state.network_profile {
            return Err(EvmConstructionError::ForkProfileMismatch {
                fork: state.backend.network_profile().name(),
                prepared: state.network_profile.name(),
            });
        }

        Ok(PreparedEvm {
            backend: state.backend.clone(),
            network_context: execution_context::<FEN>(&evm_env),
            evm_env,
            tx_env,
            fork_block_number,
            network_profile,
            is_fork: state.is_fork,
        })
    }
}

/// Opaque backend state reusable across independent EVM constructions.
#[derive(Clone, Debug)]
pub struct ReusableEvmState<FEN: FoundryEvmNetwork> {
    backend: Backend<FEN>,
    network_profile: ResolvedNetworkProfile,
    is_fork: bool,
}

impl<FEN: FoundryEvmNetwork> ReusableEvmState<FEN> {
    /// Returns the immutable network profile owned by this reusable state.
    pub const fn network_profile(&self) -> ResolvedNetworkProfile {
        self.network_profile
    }
}

/// Opaque environment and reusable state prepared for one construction.
#[derive(Clone, Debug)]
pub struct PreparedEvm<FEN: FoundryEvmNetwork> {
    backend: Backend<FEN>,
    evm_env: EvmEnvFor<FEN>,
    tx_env: TxEnvFor<FEN>,
    fork_block_number: Option<BlockNumber>,
    network_profile: ResolvedNetworkProfile,
    network_context: NetworkExecutionContext,
    is_fork: bool,
}

impl<FEN: FoundryEvmNetwork> PreparedEvm<FEN> {
    /// Returns reusable state without retaining this preparation's environment snapshot.
    pub fn reusable_state(&self) -> ReusableEvmState<FEN> {
        ReusableEvmState {
            backend: self.backend.clone(),
            network_profile: self.network_profile,
            is_fork: self.is_fork,
        }
    }

    /// Refreshes environment data while reusing this preparation's backend state and profile.
    pub async fn refresh(&self, resolved: &ResolvedEvmOpts) -> Result<Self, EvmConstructionError>
    where
        SpecFor<FEN>: Into<revm::primitives::hardfork::SpecId> + Default + Copy,
        BlockEnvFor<FEN>: FoundryBlock + Default,
        TxEnvFor<FEN>: FoundryTransaction + Default,
    {
        if resolved.network_profile() != self.network_profile {
            return Err(EvmConstructionError::ForkProfileMismatch {
                fork: self.network_profile.name(),
                prepared: resolved.network_profile().name(),
            });
        }
        let prepared = opts_construction::prepare::<SpecFor<FEN>, BlockEnvFor<FEN>, TxEnvFor<FEN>>(
            resolved.evm_opts(),
            None,
            self.network_profile,
        )
        .await
        .map_err(|error| EvmConstructionError::Environment(error.to_string()))?;
        let opts_construction::PreparedEvmOpts { evm_env, tx_env, fork_block_number, .. } =
            prepared;
        let network_context = execution_context::<FEN>(&evm_env);

        Ok(Self {
            backend: self.backend.clone(),
            evm_env,
            tx_env,
            fork_block_number,
            network_profile: self.network_profile,
            network_context,
            is_fork: resolved.has_fork(),
        })
    }

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

    /// Returns the executing chain ID when this preparation uses a remote fork.
    pub const fn fork_chain_id(&self) -> Option<u64> {
        if self.is_fork { Some(self.network_context.chain_id) } else { None }
    }

    /// Returns the prepared EVM environment for read-only consumer projections.
    pub const fn evm_env(&self) -> &EvmEnvFor<FEN> {
        &self.evm_env
    }

    /// Returns the prepared transaction environment for read-only consumer projections.
    pub const fn tx_env(&self) -> &TxEnvFor<FEN> {
        &self.tx_env
    }

    /// Applies consumer environment changes before deriving the construction snapshot.
    pub fn configure_env(
        &mut self,
        configure: impl FnOnce(&mut EvmEnvFor<FEN>, &mut TxEnvFor<FEN>),
    ) {
        configure(&mut self.evm_env, &mut self.tx_env);
        self.network_context = execution_context::<FEN>(&self.evm_env);
    }

    /// Validates precompile composition before starting consumer execution.
    pub fn validate(&self) -> Result<(), EvmConstructionError> {
        validate_precompile_composition::<FEN>(
            self.network_profile,
            self.network_context,
            &self.evm_env,
        )?;
        Ok(())
    }

    /// Builds a trace decoder bound to this preparation's execution snapshot.
    pub fn trace_decoder(&self, config: DecoderConfig) -> CallTraceDecoder {
        build_decoder(self.network_profile, self.network_context, self.fork_chain_id(), config)
    }

    /// Constructs an executor and decoder bound to one environment snapshot.
    pub fn construct(
        self,
        config: ExecutorConfig<FEN>,
    ) -> Result<ConstructedEvm<FEN>, EvmConstructionError> {
        self.validate()?;
        let Self {
            backend, evm_env, mut tx_env, network_profile, network_context, is_fork, ..
        } = self;

        let ExecutorConfig { inspectors, gas_limit, spec, legacy_assertions, fee_token, .. } =
            config;
        tx_env.set_fee_token(fee_token);
        let mut builder = ExecutorBuilder::default()
            .inspectors(|_| inspectors.network_profile(network_profile))
            .spec_id_opt(spec)
            .legacy_assertions(legacy_assertions);
        if let Some(gas_limit) = gas_limit {
            builder = builder.gas_limit(gas_limit);
        }
        let executor = builder.build(evm_env, tx_env, backend);
        let decoder = build_decoder(
            network_profile,
            network_context,
            Some(network_context.chain_id),
            DecoderConfig::default(),
        );

        Ok(ConstructedEvm {
            executor,
            decoder,
            chain_id: network_context.chain_id,
            timestamp: network_context.timestamp,
            network_profile,
            is_fork,
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
    fee_token: Option<Address>,
    marker: PhantomData<FEN>,
}

impl<FEN: FoundryEvmNetwork> Default for ExecutorConfig<FEN> {
    fn default() -> Self {
        Self {
            inspectors: InspectorStackBuilder::default(),
            gas_limit: None,
            spec: None,
            legacy_assertions: false,
            fee_token: None,
            marker: PhantomData,
        }
    }
}

impl<FEN: FoundryEvmNetwork> ExecutorConfig<FEN> {
    /// Enables log collection and optional live log output.
    pub fn logs(mut self, live_logs: bool) -> Self {
        self.inspectors = self.inspectors.logs(live_logs);
        self
    }

    /// Enables cheatcodes with the supplied consumer configuration.
    pub fn cheatcodes(mut self, config: Arc<CheatsConfig>) -> Self {
        self.inspectors = self.inspectors.cheatcodes(config);
        self
    }

    /// Adds script wallets to cheatcode execution.
    pub fn wallets(mut self, wallets: Wallets) -> Self {
        self.inspectors = self.inspectors.wallets(wallets);
        self
    }

    /// Enables or disables line coverage collection.
    pub fn line_coverage(mut self, enable: bool) -> Self {
        self.inspectors = self.inspectors.line_coverage(enable);
        self
    }

    /// Supplies source analysis used by debugger and cheatcode behavior.
    pub fn analysis(mut self, analysis: Analysis) -> Self {
        self.inspectors = self.inspectors.set_analysis(analysis);
        self
    }

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

    /// Captures Chisel stack and memory state at the requested program counter.
    pub fn chisel_state(mut self, final_pc: usize) -> Self {
        self.inspectors = self.inspectors.chisel_state(final_pc);
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

    /// Optionally overrides the EVM spec used by the executor.
    pub const fn spec_id_opt(self, spec: Option<SpecFor<FEN>>) -> Self {
        if let Some(spec) = spec { self.spec_id(spec) } else { self }
    }

    /// Sets the Tempo fee token on the transaction environment.
    pub const fn fee_token(mut self, fee_token: Option<Address>) -> Self {
        self.fee_token = fee_token;
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
    network_profile: ResolvedNetworkProfile,
    is_fork: bool,
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

    /// Returns the executor's current backend state without retaining its environment snapshot.
    pub fn reusable_state(&self) -> ReusableEvmState<FEN> {
        ReusableEvmState {
            backend: self.executor.backend().clone(),
            network_profile: self.network_profile,
            is_fork: self.is_fork,
        }
    }

    /// Decodes one call using the decoder bound to this construction.
    pub async fn decode_function(&self, trace: &CallTrace) -> DecodedCallTrace {
        self.decoder.decode_function(trace).await
    }

    /// Returns the decoder bound to this construction snapshot.
    pub const fn decoder(&self) -> &CallTraceDecoder {
        &self.decoder
    }

    /// Builds a consumer decoder bound to this construction snapshot.
    pub fn trace_decoder(&self, config: DecoderConfig) -> CallTraceDecoder {
        build_decoder(
            self.network_profile,
            NetworkExecutionContext::new(self.chain_id, self.timestamp),
            Some(self.chain_id),
            config,
        )
    }

    /// Returns the coherent executor projection for consumers that own execution.
    pub fn into_executor(self) -> Executor<FEN> {
        self.executor
    }
}

/// Consumer-specific trace decoding behavior without network selection fields.
#[derive(Default)]
#[must_use = "decoder config does nothing unless passed to a snapshot-bound trace decoder"]
pub struct DecoderConfig {
    known_contracts: Option<ContractsByArtifact>,
    labels: AddressHashMap<String>,
    label_disabled: bool,
    verbosity: u8,
    signature_identifier: Option<SignaturesIdentifier>,
    debug_identifier: Option<DebugTraceIdentifier>,
}

impl DecoderConfig {
    /// Adds locally compiled contracts and their ABIs.
    pub fn known_contracts(mut self, contracts: ContractsByArtifact) -> Self {
        self.known_contracts = Some(contracts);
        self
    }

    /// Adds address labels known by the consumer.
    pub fn labels(mut self, labels: impl IntoIterator<Item = (Address, String)>) -> Self {
        self.labels.extend(labels);
        self
    }

    /// Enables or disables address labels.
    pub const fn label_disabled(mut self, disabled: bool) -> Self {
        self.label_disabled = disabled;
        self
    }

    /// Sets trace decoding verbosity.
    pub const fn verbosity(mut self, verbosity: u8) -> Self {
        self.verbosity = verbosity;
        self
    }

    /// Adds external function and event signature identification.
    pub fn signature_identifier(mut self, identifier: SignaturesIdentifier) -> Self {
        self.signature_identifier = Some(identifier);
        self
    }

    /// Adds internal source-level trace identification.
    pub fn debug_identifier(mut self, identifier: DebugTraceIdentifier) -> Self {
        self.debug_identifier = Some(identifier);
        self
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

fn build_decoder(
    network_profile: ResolvedNetworkProfile,
    network_context: NetworkExecutionContext,
    chain_id: Option<u64>,
    config: DecoderConfig,
) -> CallTraceDecoder {
    let DecoderConfig {
        known_contracts,
        labels,
        label_disabled,
        verbosity,
        signature_identifier,
        debug_identifier,
    } = config;
    let mut builder = CallTraceDecoderBuilder::new()
        .with_labels(labels)
        .with_label_disabled(label_disabled)
        .with_verbosity(verbosity)
        .with_chain_id(chain_id);
    if let Some(known_contracts) = &known_contracts {
        builder = builder.with_known_contracts(known_contracts);
    }
    if let Some(identifier) = signature_identifier {
        builder = builder.with_signature_identifier(identifier);
    }
    if let Some(identifier) = debug_identifier {
        builder = builder.with_debug_identifier(identifier);
    }
    bind_network_snapshot(builder.build(), network_profile, network_context)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "hashkey")]
    use alloy_primitives::address;
    #[cfg(feature = "hashkey")]
    use foundry_config::GasLimit;
    #[cfg(feature = "hashkey")]
    use foundry_evm_core::evm::OpEvmNetwork;
    use foundry_evm_core::{
        evm::EthEvmNetwork,
        opts::{
            EvmOpts,
            resolution::{CommandProfileResolution, NetworkIntent},
        },
    };
    use foundry_evm_networks::NetworkConfigs;

    fn resolve(evm_opts: EvmOpts) -> ResolvedEvmOpts {
        CommandProfileResolution::new().resolve_evm_opts(evm_opts, NetworkIntent::new()).unwrap()
    }

    #[cfg(feature = "hashkey")]
    const B20_FACTORY: Address = address!("B20F000000000000000000000000000000000000");

    #[tokio::test]
    async fn rejects_profile_family_mismatch_before_preparation() {
        let mut evm_opts = EvmOpts::default();
        evm_opts.networks = NetworkConfigs::with_optimism();
        let error =
            match EvmConstruction::prepare::<EthEvmNetwork>(&resolve(evm_opts), &Config::default())
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
    async fn rejects_reusable_state_from_another_resolved_profile() {
        let mut hashkey_opts = EvmOpts::default();
        hashkey_opts.networks = NetworkConfigs::with_hashkey();
        let hashkey = resolve(hashkey_opts);
        let state = EvmConstruction::prepare::<OpEvmNetwork>(&hashkey, &Config::default())
            .await
            .unwrap()
            .reusable_state();

        let ethereum = resolve(EvmOpts::default());
        let error = EvmConstruction::prepare_with_state::<OpEvmNetwork>(&ethereum, &state)
            .await
            .unwrap_err();
        assert!(matches!(error, EvmConstructionError::ForkProfileMismatch { .. }));
    }

    #[cfg(feature = "hashkey")]
    #[tokio::test(flavor = "multi_thread")]
    async fn construction_binds_execution_and_decoder_to_one_snapshot() {
        let mut evm_opts = EvmOpts::default();
        evm_opts.env.chain_id = Some(177);
        evm_opts.env.gas_limit = GasLimit(30_000_000);
        evm_opts.networks = NetworkConfigs::with_hashkey();
        let resolved = resolve(evm_opts.clone());

        let normal = EvmConstruction::prepare::<OpEvmNetwork>(&resolved, &Config::default())
            .await
            .unwrap()
            .construct(ExecutorConfig::default())
            .unwrap();
        let traced = EvmConstruction::prepare::<OpEvmNetwork>(&resolved, &Config::default())
            .await
            .unwrap()
            .construct(ExecutorConfig::default().trace_mode(TraceMode::Call))
            .unwrap();
        let isolated = EvmConstruction::prepare::<OpEvmNetwork>(&resolved, &Config::default())
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

    #[cfg(feature = "hashkey")]
    #[tokio::test(flavor = "multi_thread")]
    async fn reusable_state_derives_a_fresh_activation_snapshot() {
        let mut evm_opts = EvmOpts::default();
        evm_opts.env.chain_id = Some(177);
        evm_opts.env.gas_limit = GasLimit(30_000_000);
        evm_opts.env.block_timestamp = revm::primitives::U256::ZERO;

        evm_opts.networks = NetworkConfigs::with_hashkey();
        let resolved = resolve(evm_opts.clone());
        let prepared =
            EvmConstruction::prepare::<OpEvmNetwork>(&resolved, &Config::default()).await.unwrap();
        let state = prepared.reusable_state();
        let before = prepared.construct(ExecutorConfig::default()).unwrap();
        let trace = CallTrace { address: B20_FACTORY, ..Default::default() };
        assert_eq!(before.timestamp(), 0);
        assert_eq!(before.decode_function(&trace).await.label.as_deref(), Some("B20Factory"));

        evm_opts.env.block_timestamp = revm::primitives::U256::from(1);
        let resolved = resolve(evm_opts);
        let after = EvmConstruction::prepare_with_state::<OpEvmNetwork>(&resolved, &state)
            .await
            .unwrap()
            .construct(ExecutorConfig::default())
            .unwrap();

        assert_eq!(after.timestamp(), 1);
        assert_eq!(after.decode_function(&trace).await.label.as_deref(), Some("B20Factory"));
    }
}
