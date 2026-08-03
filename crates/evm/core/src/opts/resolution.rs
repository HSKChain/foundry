//! Canonical command-owned network profile resolution.

use super::EvmOpts;
use crate::{EvmEnv, FoundryBlock, FoundryTransaction, fork::CreateFork};
use alloy_network::AnyNetwork;
use alloy_primitives::{BlockNumber, ChainId};
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_types::anvil::NodeInfo;
use foundry_config::Config;
use foundry_evm_networks::{EvmFamily, NetworkConfigs, NetworkVariant, ResolvedNetworkProfile};
use revm::primitives::hardfork::SpecId;
use std::fmt;
use thiserror::Error;

/// A network profile that can be required by command-local normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileKind {
    /// Plain Ethereum execution.
    Ethereum,
    /// Ethereum execution with the Celo extension.
    Celo,
    /// OP Stack execution.
    Optimism,
    /// Tempo execution.
    Tempo,
    /// HashKey B20 execution.
    #[cfg(feature = "hashkey")]
    HashKey,
}

impl ProfileKind {
    fn configs(self) -> NetworkConfigs {
        match self {
            Self::Ethereum => NetworkConfigs::from(NetworkVariant::Ethereum),
            Self::Celo => NetworkConfigs::with_celo(),
            Self::Optimism => NetworkConfigs::with_optimism(),
            Self::Tempo => NetworkConfigs::with_tempo(),
            #[cfg(feature = "hashkey")]
            Self::HashKey => NetworkConfigs::with_hashkey(),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Ethereum => "ethereum",
            Self::Celo => "celo",
            Self::Optimism => "optimism",
            Self::Tempo => "tempo",
            #[cfg(feature = "hashkey")]
            Self::HashKey => "hashkey",
        }
    }

    fn matches(self, profile: ResolvedNetworkProfile) -> bool {
        match self {
            Self::Ethereum => profile.name() == "ethereum",
            Self::Celo => profile.is_celo(),
            Self::Optimism => profile.name() == "optimism",
            Self::Tempo => profile.is_tempo(),
            #[cfg(feature = "hashkey")]
            Self::HashKey => profile.is_hashkey(),
        }
    }
}

impl fmt::Display for ProfileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Stable source labels for command requirements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkRequirementSource {
    /// A Tempo fee-token option.
    TempoFeeToken,
    /// A Tempo transaction option.
    TempoTransaction,
    /// A hardfork constraint.
    Hardfork,
    /// A caller-defined requirement with a stable static label.
    Other(&'static str),
}

impl fmt::Display for NetworkRequirementSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::TempoFeeToken => "--tempo.fee-token",
            Self::TempoTransaction => "Tempo transaction",
            Self::Hardfork => "hardfork",
            Self::Other(label) => label,
        };
        f.write_str(label)
    }
}

/// A normalized command requirement for an exact network profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactProfileRequirement {
    /// Required profile.
    pub profile: ProfileKind,
    /// Stable source used in diagnostics.
    pub source: NetworkRequirementSource,
}

/// A normalized command requirement for an EVM family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvmFamilyConstraint {
    /// Required EVM family.
    pub family: EvmFamily,
    /// Stable source used in diagnostics.
    pub source: NetworkRequirementSource,
}

/// Network facts normalized by a command before profile resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkIntent {
    exact_profiles: Vec<ExactProfileRequirement>,
    family_constraints: Vec<EvmFamilyConstraint>,
    chain_hint: Option<ChainId>,
    fork_identity_requested: bool,
}

impl NetworkIntent {
    /// Creates an intent with no additional network evidence.
    pub const fn new() -> Self {
        Self {
            exact_profiles: Vec::new(),
            family_constraints: Vec::new(),
            chain_hint: None,
            fork_identity_requested: false,
        }
    }

    /// Adds an exact profile requirement.
    pub fn require_profile(
        mut self,
        profile: ProfileKind,
        source: NetworkRequirementSource,
    ) -> Self {
        self.exact_profiles.push(ExactProfileRequirement { profile, source });
        self
    }

    /// Adds an EVM family constraint.
    pub fn require_family(mut self, family: EvmFamily, source: NetworkRequirementSource) -> Self {
        self.family_constraints.push(EvmFamilyConstraint { family, source });
        self
    }

    /// Adds an identity-bearing chain hint.
    pub const fn with_chain_hint(mut self, chain_id: ChainId) -> Self {
        self.chain_hint = Some(chain_id);
        self
    }

    /// Requests fork identity when local evidence does not select a profile.
    pub const fn with_fork_identity(mut self) -> Self {
        self.fork_identity_requested = true;
        self
    }
}

/// Identity returned by a fork endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkIdentity {
    /// Chain ID returned by `eth_chainId`.
    pub chain_id: ChainId,
    /// Optional network marker returned by an Anvil node-info method.
    pub node_network: Option<String>,
}

impl ForkIdentity {
    /// Creates a fork identity without an optional node marker.
    pub const fn new(chain_id: ChainId) -> Self {
        Self { chain_id, node_network: None }
    }

    /// Adds an optional node network marker.
    pub fn with_node_network(mut self, network: impl Into<String>) -> Self {
        self.node_network = Some(network.into());
        self
    }
}

/// Errors produced while obtaining fork identity.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ForkIdentityError {
    /// The required identity transport was unavailable.
    #[error("fork identity transport unavailable: {0}")]
    Unavailable(String),
}

/// Errors returned by canonical command profile resolution.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum NetworkResolutionError {
    /// An exact requirement conflicts with the selected profile.
    #[error(
        "network requirement `{required}` from `{requirement_source}` conflicts with configured network `{configured}`"
    )]
    ConflictingRequirement {
        /// Existing or selected profile.
        configured: &'static str,
        /// Required profile.
        required: &'static str,
        /// Requirement source.
        requirement_source: NetworkRequirementSource,
    },
    /// A family constraint conflicts with the selected profile.
    #[error(
        "EVM family requirement `{required}` from `{requirement_source}` conflicts with selected network `{selected}`"
    )]
    ConflictingEvmFamily {
        /// Selected profile.
        selected: &'static str,
        /// Required family.
        required: &'static str,
        /// Requirement source.
        requirement_source: NetworkRequirementSource,
    },
    /// Required fork identity could not be obtained.
    #[error("failed to resolve network profile from fork identity: {source}")]
    ForkIdentityUnavailable {
        /// Underlying identity error.
        source: ForkIdentityError,
    },
}

/// A fork identity source used by the synchronous resolver.
pub trait ForkIdentitySource {
    /// Returns fork identity, or a typed transport error.
    fn resolve(&mut self) -> Result<Option<ForkIdentity>, ForkIdentityError>;
}

/// An asynchronous fork identity source used by production RPC adapters.
pub trait AsyncForkIdentitySource {
    /// Returns fork identity, or a typed transport error.
    async fn resolve(&mut self) -> Result<Option<ForkIdentity>, ForkIdentityError>;
}

/// A production JSON-RPC adapter for fork identity resolution.
///
/// The provider is constructed from the fork options, but no request is made until the resolver
/// determines that fork identity is required. Provider construction and transport errors are
/// converted to stable messages so credentials, headers, and provider internals cannot escape in
/// user-facing diagnostics.
pub struct RpcForkIdentitySource {
    provider: Option<RootProvider<AnyNetwork>>,
    initialization_error: Option<ForkIdentityError>,
}

impl RpcForkIdentitySource {
    /// Creates an adapter from fork options without making an RPC request.
    pub fn from_evm_opts(evm_opts: &EvmOpts) -> Self {
        let Some(url) = evm_opts.fork_url.as_deref() else {
            return Self { provider: None, initialization_error: None };
        };

        match evm_opts.fork_provider_with_url::<AnyNetwork>(url) {
            Ok(provider) => Self { provider: Some(provider), initialization_error: None },
            Err(_) => Self {
                provider: None,
                initialization_error: Some(ForkIdentityError::Unavailable(
                    "failed to create fork identity provider".to_string(),
                )),
            },
        }
    }

    /// Creates an adapter around an already-configured provider.
    pub const fn from_provider(provider: RootProvider<AnyNetwork>) -> Self {
        Self { provider: Some(provider), initialization_error: None }
    }
}

impl fmt::Debug for RpcForkIdentitySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcForkIdentitySource").finish_non_exhaustive()
    }
}

impl AsyncForkIdentitySource for RpcForkIdentitySource {
    async fn resolve(&mut self) -> Result<Option<ForkIdentity>, ForkIdentityError> {
        if let Some(error) = &self.initialization_error {
            return Err(error.clone());
        }

        let Some(provider) = &self.provider else {
            // No fork endpoint is configured: there is no identity evidence to request. The
            // resolver treats this as a clean fallback to the configured or default profile.
            return Ok(None);
        };

        let chain_id = provider.get_chain_id().await.map_err(|_| {
            ForkIdentityError::Unavailable("eth_chainId request failed".to_string())
        })?;

        let node_network = if chain_id == 31337 {
            provider
                .raw_request::<_, NodeInfo>("anvil_nodeInfo".into(), ())
                .await
                .ok()
                .and_then(|info| info.network)
        } else {
            None
        };

        Ok(Some(ForkIdentity { chain_id, node_network }))
    }
}

/// A source used when no fork identity lookup is configured.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoForkIdentity;

impl ForkIdentitySource for NoForkIdentity {
    fn resolve(&mut self) -> Result<Option<ForkIdentity>, ForkIdentityError> {
        Ok(None)
    }
}

/// Deterministic fork identity source for resolver tests and local adapters.
#[derive(Clone, Debug, Default)]
pub struct InMemoryForkIdentitySource {
    identity: Option<ForkIdentity>,
    error: Option<ForkIdentityError>,
    calls: usize,
}

impl InMemoryForkIdentitySource {
    /// Creates a source returning the supplied identity.
    pub fn new(identity: ForkIdentity) -> Self {
        Self { identity: Some(identity), ..Default::default() }
    }

    /// Creates a source returning no identity.
    pub const fn empty() -> Self {
        Self { identity: None, error: None, calls: 0 }
    }

    /// Creates a source returning a typed transport failure.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            identity: None,
            error: Some(ForkIdentityError::Unavailable(message.into())),
            calls: 0,
        }
    }

    /// Returns the number of identity lookups performed.
    pub const fn calls(&self) -> usize {
        self.calls
    }
}

impl ForkIdentitySource for InMemoryForkIdentitySource {
    fn resolve(&mut self) -> Result<Option<ForkIdentity>, ForkIdentityError> {
        self.calls += 1;
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        Ok(self.identity.clone())
    }
}

impl AsyncForkIdentitySource for InMemoryForkIdentitySource {
    async fn resolve(&mut self) -> Result<Option<ForkIdentity>, ForkIdentityError> {
        self.calls += 1;
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        Ok(self.identity.clone())
    }
}

/// An opaque, immutable result of command profile resolution.
#[derive(Clone, Debug)]
pub struct ResolvedEvmOpts {
    evm_opts: EvmOpts,
    network_profile: ResolvedNetworkProfile,
}

impl ResolvedEvmOpts {
    /// Returns the immutable profile selected for the command.
    pub const fn network_profile(&self) -> ResolvedNetworkProfile {
        self.network_profile
    }

    /// Returns whether the resolved command retains a fork URL.
    pub const fn has_fork(&self) -> bool {
        self.evm_opts.fork_url.is_some()
    }

    /// Returns the read-only options projection used by construction and consumer adapters.
    #[doc(hidden)]
    pub const fn evm_opts(&self) -> &EvmOpts {
        &self.evm_opts
    }

    /// Updates the fork endpoint without changing the resolved network profile.
    #[doc(hidden)]
    pub fn with_fork_url(mut self, fork_url: Option<String>) -> Self {
        self.evm_opts.fork_url = fork_url;
        self
    }

    /// Updates the pinned fork block without changing the resolved network profile.
    #[doc(hidden)]
    pub const fn with_fork_block_number(mut self, fork_block_number: Option<u64>) -> Self {
        self.evm_opts.fork_block_number = fork_block_number;
        self
    }

    /// Updates the transaction sender without changing the resolved network profile.
    #[doc(hidden)]
    pub const fn with_sender(mut self, sender: alloy_primitives::Address) -> Self {
        self.evm_opts.sender = sender;
        self
    }

    /// Prepares environment, transaction, and optional initial-fork material atomically from
    /// this resolved profile.
    ///
    /// When `config` is supplied, fork material pins the resolved fork block so subsequent fork
    /// operations use the same block. On some L2s (e.g., Arbitrum) `block_env.number` is remapped
    /// to the L1 block number, so `fork_block_number` carries the original L2 block value.
    pub async fn prepare<
        SPEC: Into<SpecId> + Default + Copy,
        BLOCK: FoundryBlock + Default,
        TX: FoundryTransaction + Default,
    >(
        &self,
        config: Option<&Config>,
    ) -> eyre::Result<PreparedEvmOpts<SPEC, BLOCK, TX>> {
        let (evm_env, tx_env, fork_block_number) =
            self.evm_opts.env_for_profile(self.network_profile).await?;
        let fork = config.and_then(|config| {
            self.evm_opts.get_fork_for_profile(
                config,
                evm_env.cfg_env.chain_id,
                fork_block_number,
                self.network_profile,
            )
        });
        Ok(PreparedEvmOpts { evm_env, tx_env, fork_block_number, fork })
    }
}

/// Environment, transaction, and optional initial-fork material prepared atomically from one
/// resolved profile.
pub struct PreparedEvmOpts<SPEC, BLOCK, TX> {
    /// Prepared environment.
    pub evm_env: EvmEnv<SPEC, BLOCK>,
    /// Prepared transaction environment.
    pub tx_env: TX,
    /// Actual fork block number resolved from the fork endpoint, if forked.
    pub fork_block_number: Option<BlockNumber>,
    /// Initial fork material, when forked and a config was supplied.
    pub fork: Option<CreateFork>,
}

/// Canonical command profile resolver.
#[derive(Clone, Debug)]
pub struct CommandProfileResolution<S = NoForkIdentity> {
    fork_identity: S,
}

impl Default for CommandProfileResolution<NoForkIdentity> {
    fn default() -> Self {
        Self { fork_identity: NoForkIdentity }
    }
}

impl CommandProfileResolution<NoForkIdentity> {
    /// Creates a resolver that falls back to Ethereum without fork identity evidence.
    pub const fn new() -> Self {
        Self { fork_identity: NoForkIdentity }
    }
}

impl<S> CommandProfileResolution<S> {
    /// Creates a resolver backed by the supplied fork identity source.
    pub const fn with_fork_identity_source(fork_identity: S) -> Self {
        Self { fork_identity }
    }
}

impl<S: ForkIdentitySource> CommandProfileResolution<S> {
    /// Resolves one command's normalized intent into an opaque EVM options carrier.
    pub fn resolve_evm_opts(
        &mut self,
        evm_opts: EvmOpts,
        intent: NetworkIntent,
    ) -> Result<ResolvedEvmOpts, NetworkResolutionError> {
        let identity = if should_resolve_fork_identity(&evm_opts, &intent) {
            Some(
                self.fork_identity
                    .resolve()
                    .map_err(|source| NetworkResolutionError::ForkIdentityUnavailable { source })?,
            )
        } else {
            None
        };
        resolve_evm_opts_with_identity(evm_opts, intent, identity.flatten())
    }
}

impl<S: AsyncForkIdentitySource> CommandProfileResolution<S> {
    /// Resolves one command's normalized intent using an asynchronous identity source.
    pub async fn resolve_evm_opts_async(
        &mut self,
        evm_opts: EvmOpts,
        intent: NetworkIntent,
    ) -> Result<ResolvedEvmOpts, NetworkResolutionError> {
        let identity = if should_resolve_fork_identity(&evm_opts, &intent) {
            Some(
                self.fork_identity
                    .resolve()
                    .await
                    .map_err(|source| NetworkResolutionError::ForkIdentityUnavailable { source })?,
            )
        } else {
            None
        };
        resolve_evm_opts_with_identity(evm_opts, intent, identity.flatten())
    }
}

fn should_resolve_fork_identity(evm_opts: &EvmOpts, intent: &NetworkIntent) -> bool {
    let mut selected = evm_opts.networks.has_explicit_selection();
    selected |= !intent.exact_profiles.is_empty();
    selected |= intent
        .chain_hint
        .is_some_and(|chain_id| NetworkConfigs::from_known_chain_id(chain_id).is_some());

    !selected && (intent.fork_identity_requested || evm_opts.fork_url.is_some())
}

fn resolve_evm_opts_with_identity(
    evm_opts: EvmOpts,
    intent: NetworkIntent,
    identity: Option<ForkIdentity>,
) -> Result<ResolvedEvmOpts, NetworkResolutionError> {
    let configured = evm_opts.networks;
    let mut selected = configured.has_explicit_selection().then_some(configured.resolve());

    for requirement in &intent.exact_profiles {
        if let Some(profile) = selected {
            if !requirement.profile.matches(profile) {
                return Err(NetworkResolutionError::ConflictingRequirement {
                    configured: profile.name(),
                    required: requirement.profile.name(),
                    requirement_source: requirement.source,
                });
            }
        } else {
            selected = Some(requirement.profile.configs().resolve());
        }
    }

    if selected.is_none()
        && let Some(chain_id) = intent.chain_hint
        && let Some(configs) = NetworkConfigs::from_known_chain_id(chain_id)
    {
        selected = Some(configs.resolve());
    }

    if selected.is_none()
        && let Some(identity) = identity
    {
        selected = fork_profile(identity);
    }

    let network_profile = selected
        .or_else(|| {
            intent
                .family_constraints
                .first()
                .map(|constraint| family_configs(constraint.family).resolve())
        })
        .unwrap_or_default();

    for constraint in &intent.family_constraints {
        if network_profile.evm_family() != constraint.family {
            return Err(NetworkResolutionError::ConflictingEvmFamily {
                selected: network_profile.name(),
                required: constraint.family.name(),
                requirement_source: constraint.source,
            });
        }
    }

    Ok(ResolvedEvmOpts { evm_opts, network_profile })
}

fn family_configs(family: EvmFamily) -> NetworkConfigs {
    match family {
        EvmFamily::Ethereum => NetworkConfigs::default(),
        EvmFamily::Optimism => NetworkConfigs::with_optimism(),
        EvmFamily::Tempo => NetworkConfigs::with_tempo(),
    }
}

fn fork_profile(identity: ForkIdentity) -> Option<ResolvedNetworkProfile> {
    if identity.chain_id == 31337
        && identity.node_network.as_deref().is_some_and(|network| network == "tempo")
    {
        return Some(NetworkConfigs::with_tempo().resolve());
    }
    NetworkConfigs::from_known_chain_id(identity.chain_id).map(NetworkConfigs::resolve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use alloy_rpc_client::RpcClient;
    use alloy_transport::mock::Asserter;

    fn opts() -> EvmOpts {
        EvmOpts::default()
    }

    #[test]
    fn explicit_ethereum_is_authoritative_and_skips_fork_identity() {
        let mut evm_opts = opts();
        evm_opts.networks = NetworkConfigs::from(NetworkVariant::Ethereum);
        let source =
            InMemoryForkIdentitySource::new(ForkIdentity::new(4242).with_node_network("tempo"));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(evm_opts, NetworkIntent::new().with_fork_identity())
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "ethereum");
        assert_eq!(resolution.fork_identity.calls(), 0);
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn exact_requirement_conflicts_with_explicit_hashkey() {
        let mut evm_opts = opts();
        evm_opts.networks = NetworkConfigs::with_hashkey();
        let mut resolution = CommandProfileResolution::default();

        let error = resolution
            .resolve_evm_opts(
                evm_opts,
                NetworkIntent::new().require_profile(
                    ProfileKind::Tempo,
                    NetworkRequirementSource::TempoTransaction,
                ),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            NetworkResolutionError::ConflictingRequirement {
                configured: "hashkey",
                required: "tempo",
                requirement_source: NetworkRequirementSource::TempoTransaction,
            }
        ));
    }

    #[test]
    fn exact_requirement_selects_profile_before_fork_identity() {
        let source = InMemoryForkIdentitySource::unavailable("must not be called");
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new()
                    .require_profile(ProfileKind::Tempo, NetworkRequirementSource::TempoTransaction)
                    .with_fork_identity(),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
        assert_eq!(resolution.fork_identity.calls(), 0);
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn optimism_family_constraint_preserves_explicit_hashkey() {
        let mut evm_opts = opts();
        evm_opts.networks = NetworkConfigs::with_hashkey();
        let mut resolution = CommandProfileResolution::default();

        let resolved = resolution
            .resolve_evm_opts(
                evm_opts,
                NetworkIntent::new()
                    .require_family(EvmFamily::Optimism, NetworkRequirementSource::Hardfork),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "hashkey");
    }

    #[test]
    fn known_chain_hint_selects_profile_without_fork_lookup() {
        let source = InMemoryForkIdentitySource::unavailable("must not be called");
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new().with_chain_hint(42431).with_fork_identity(),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
        assert_eq!(resolution.fork_identity.calls(), 0);
    }

    #[test]
    fn known_optimism_chain_hint_selects_optimism() {
        let source = InMemoryForkIdentitySource::unavailable("must not be called");
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(opts(), NetworkIntent::new().with_chain_hint(10).with_fork_identity())
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "optimism");
        assert_eq!(resolution.fork_identity.calls(), 0);
    }

    #[test]
    fn unknown_chain_hint_can_select_tempo_from_fork_identity() {
        let source = InMemoryForkIdentitySource::new(ForkIdentity::new(42431));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new().with_chain_hint(999_999).with_fork_identity(),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
        assert_eq!(resolution.fork_identity.calls(), 1);
    }

    #[test]
    fn unknown_chain_hint_uses_known_fork_identity() {
        let source = InMemoryForkIdentitySource::new(ForkIdentity::new(10));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new().with_chain_hint(999_999).with_fork_identity(),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "optimism");
        assert_eq!(resolution.fork_identity.calls(), 1);
    }

    #[test]
    fn family_constraint_is_checked_after_chain_hint_inference() {
        let source = InMemoryForkIdentitySource::unavailable("must not be called");
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let error = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new()
                    .with_chain_hint(42431)
                    .require_family(EvmFamily::Optimism, NetworkRequirementSource::Hardfork),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            NetworkResolutionError::ConflictingEvmFamily {
                selected: "tempo",
                required: "optimism",
                requirement_source: NetworkRequirementSource::Hardfork,
            }
        ));
        assert_eq!(resolution.fork_identity.calls(), 0);
    }

    #[test]
    fn family_constraint_is_checked_after_fork_inference() {
        let source = InMemoryForkIdentitySource::new(ForkIdentity::new(42431));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let error = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new()
                    .with_fork_identity()
                    .require_family(EvmFamily::Optimism, NetworkRequirementSource::Hardfork),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            NetworkResolutionError::ConflictingEvmFamily {
                selected: "tempo",
                required: "optimism",
                requirement_source: NetworkRequirementSource::Hardfork,
            }
        ));
    }

    #[test]
    fn multiple_exact_requirements_conflict_instead_of_overwriting() {
        let mut resolution = CommandProfileResolution::default();

        let error = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new()
                    .require_profile(ProfileKind::Tempo, NetworkRequirementSource::TempoTransaction)
                    .require_profile(ProfileKind::Optimism, NetworkRequirementSource::Hardfork),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            NetworkResolutionError::ConflictingRequirement {
                configured: "tempo",
                required: "optimism",
                requirement_source: NetworkRequirementSource::Hardfork,
            }
        ));
    }

    #[test]
    fn ethereum_family_constraint_preserves_celo_hint() {
        let source = InMemoryForkIdentitySource::unavailable("must not be called");
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new()
                    .with_chain_hint(42220)
                    .require_family(EvmFamily::Ethereum, NetworkRequirementSource::Hardfork),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "celo");
        assert_eq!(resolution.fork_identity.calls(), 0);
    }

    #[test]
    fn anvil_tempo_marker_selects_tempo() {
        let source =
            InMemoryForkIdentitySource::new(ForkIdentity::new(31337).with_node_network("tempo"));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved =
            resolution.resolve_evm_opts(opts(), NetworkIntent::new().with_fork_identity()).unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
    }

    #[test]
    fn unknown_fork_identity_falls_back_to_ethereum() {
        let source = InMemoryForkIdentitySource::new(ForkIdentity::new(999_999));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved =
            resolution.resolve_evm_opts(opts(), NetworkIntent::new().with_fork_identity()).unwrap();

        assert_eq!(resolved.network_profile().name(), "ethereum");
    }

    #[test]
    fn known_celo_hint_selects_celo() {
        let source = InMemoryForkIdentitySource::unavailable("must not be called");
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts(
                opts(),
                NetworkIntent::new().with_chain_hint(42220).with_fork_identity(),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "celo");
        assert_eq!(resolution.fork_identity.calls(), 0);
    }

    #[test]
    fn fork_url_requests_identity_without_caller_flag() {
        let mut evm_opts = opts();
        evm_opts.fork_url = Some("http://localhost:8545".to_string());
        let source = InMemoryForkIdentitySource::new(ForkIdentity::new(10));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution.resolve_evm_opts(evm_opts, NetworkIntent::new()).unwrap();

        assert_eq!(resolved.network_profile().name(), "optimism");
        assert_eq!(resolution.fork_identity.calls(), 1);
    }

    #[test]
    fn required_fork_identity_failure_is_typed() {
        let source = InMemoryForkIdentitySource::unavailable("eth_chainId failed");
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let error = resolution
            .resolve_evm_opts(opts(), NetworkIntent::new().with_fork_identity())
            .unwrap_err();

        assert!(matches!(
            error,
            NetworkResolutionError::ForkIdentityUnavailable {
                source: ForkIdentityError::Unavailable(message),
            } if message == "eth_chainId failed"
        ));
    }

    #[test]
    fn carrier_is_opaque_and_retains_fork_projection() {
        let mut evm_opts = opts();
        evm_opts.fork_url = Some("http://localhost:8545".to_string());
        evm_opts.env.block_timestamp = U256::from(7);
        let mut resolution = CommandProfileResolution::default();

        let resolved = resolution.resolve_evm_opts(evm_opts, NetworkIntent::new()).unwrap();

        assert!(resolved.has_fork());
        assert_eq!(resolved.network_profile().name(), "ethereum");
    }

    #[tokio::test]
    async fn rpc_adapter_without_fork_url_has_no_identity() {
        let source = RpcForkIdentitySource::from_evm_opts(&opts());
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        // Plain execution (e.g. `forge test` without a fork URL or explicit network selection)
        // must fall back to Ethereum instead of failing on a missing identity transport.
        let resolved = resolution
            .resolve_evm_opts_async(opts(), NetworkIntent::new().with_fork_identity())
            .await
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "ethereum");
    }

    #[tokio::test]
    async fn rpc_adapter_reads_tempo_anvil_marker() {
        let (_api, handle) = anvil::spawn(anvil::NodeConfig::test_tempo()).await;
        let mut evm_opts = opts();
        evm_opts.fork_url = Some(handle.http_endpoint());
        let source = RpcForkIdentitySource::from_evm_opts(&evm_opts);
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved =
            resolution.resolve_evm_opts_async(evm_opts, NetworkIntent::new()).await.unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
    }

    #[tokio::test]
    async fn rpc_adapter_treats_unsupported_node_info_as_non_fatal() {
        let asserter = Asserter::new();
        asserter.push_success(&"0x7a69");
        let source =
            RpcForkIdentitySource::from_provider(RootProvider::new(RpcClient::mocked(asserter)));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let resolved = resolution
            .resolve_evm_opts_async(opts(), NetworkIntent::new().with_fork_identity())
            .await
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "ethereum");
    }

    #[tokio::test]
    async fn rpc_adapter_returns_redacted_chain_id_failure() {
        let asserter = Asserter::new();
        asserter.push_failure_msg("provider token=super-secret");
        let source =
            RpcForkIdentitySource::from_provider(RootProvider::new(RpcClient::mocked(asserter)));
        let mut resolution = CommandProfileResolution::with_fork_identity_source(source);

        let error = resolution
            .resolve_evm_opts_async(opts(), NetworkIntent::new().with_fork_identity())
            .await
            .unwrap_err();

        let error_text = error.to_string();
        assert!(matches!(
            error,
            NetworkResolutionError::ForkIdentityUnavailable {
                source: ForkIdentityError::Unavailable(message),
            } if message == "eth_chainId request failed"
        ));
        assert!(!error_text.contains("super-secret"));
    }
}
