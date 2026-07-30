//! # foundry-evm-networks
//!
//! Foundry EVM network configuration.

#[cfg(feature = "hashkey")]
use crate::b20_addresses::{B20_ACTIVATION_REGISTRY, B20_FACTORY, B20_POLICY_REGISTRY};
use crate::celo::transfer::{
    CELO_TRANSFER_ADDRESS, CELO_TRANSFER_LABEL, PRECOMPILE_ID_CELO_TRANSFER,
};
use alloy_chains::{
    Chain, NamedChain,
    NamedChain::{Chiado, Gnosis, Moonbase, Moonbeam, MoonbeamDev, Moonriver, Rsk, RskTestnet},
};
use alloy_eips::eip1559::BaseFeeParams;
#[cfg(feature = "hashkey")]
use alloy_evm::precompiles::Precompile;
use alloy_evm::precompiles::PrecompilesMap;
use alloy_op_hardforks::{OpChainHardforks, OpHardforks};
use alloy_primitives::{Address, ChainId, map::AddressHashMap};
use clap::Parser;
use foundry_evm_hardforks::{FoundryHardfork, TempoHardfork};
#[cfg(feature = "hashkey")]
use hsk_b20_config::B20Config;
#[cfg(feature = "hashkey")]
use hsk_b20_precompiles::{
    ActivationRegistry, B20Factory, B20Spec, BerylLookup, NoopPrecompileCallObserver,
    PolicyRegistryPrecompile,
};
use revm::precompile::PrecompileId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tempo_contracts::precompiles::{
    ACCOUNT_KEYCHAIN_ADDRESS, ADDRESS_REGISTRY_ADDRESS, NONCE_PRECOMPILE_ADDRESS,
    RECEIVE_POLICY_GUARD_ADDRESS, SIGNATURE_VERIFIER_ADDRESS, STABLECOIN_DEX_ADDRESS,
    STORAGE_CREDITS_ADDRESS, TIP_FEE_MANAGER_ADDRESS, TIP20_CHANNEL_RESERVE_ADDRESS,
    TIP20_FACTORY_ADDRESS, TIP403_REGISTRY_ADDRESS, VALIDATOR_CONFIG_ADDRESS,
    VALIDATOR_CONFIG_V2_ADDRESS,
};

pub mod celo;

/// HashKey B20 standalone local development activation admin.
///
/// This deterministic non-zero address is used only for standalone local simulation. It is not a
/// production HashKey parameter.
#[cfg(feature = "hashkey")]
pub const HSK_B20_LOCAL_ADMIN: Address =
    alloy_primitives::address!("CB00000000000000000000000000000000000000");

/// B20 singleton addresses.
#[cfg(feature = "hashkey")]
mod b20_addresses {
    use alloy_primitives::Address;

    /// `B20Factory` singleton precompile address.
    pub const B20_FACTORY: Address =
        alloy_primitives::address!("B20F000000000000000000000000000000000000");
    /// `ActivationRegistry` singleton precompile address.
    pub const B20_ACTIVATION_REGISTRY: Address =
        alloy_primitives::address!("8453000000000000000000000000000000000001");
    /// `PolicyRegistry` singleton precompile address.
    pub const B20_POLICY_REGISTRY: Address =
        alloy_primitives::address!("8453000000000000000000000000000000000002");
}

const TEMPO_PRECOMPILES: &[(&str, Address)] = &[
    ("Nonce", NONCE_PRECOMPILE_ADDRESS),
    ("StablecoinDex", STABLECOIN_DEX_ADDRESS),
    ("TIP20Factory", TIP20_FACTORY_ADDRESS),
    ("TIP403Registry", TIP403_REGISTRY_ADDRESS),
    ("FeeManager", TIP_FEE_MANAGER_ADDRESS),
    ("ValidatorConfig", VALIDATOR_CONFIG_ADDRESS),
    ("ValidatorConfigV2", VALIDATOR_CONFIG_V2_ADDRESS),
    ("AccountKeychain", ACCOUNT_KEYCHAIN_ADDRESS),
    ("SignatureVerifier", SIGNATURE_VERIFIER_ADDRESS),
    ("AddressRegistry", ADDRESS_REGISTRY_ADDRESS),
    ("TIP20ChannelReserve", TIP20_CHANNEL_RESERVE_ADDRESS),
    ("ReceivePolicyGuard", RECEIVE_POLICY_GUARD_ADDRESS),
    ("StorageCredits", STORAGE_CREDITS_ADDRESS),
];

/// Returns whether a well-known Tempo precompile address is active at `hardfork`.
pub fn is_tempo_precompile_active_at(address: Address, hardfork: TempoHardfork) -> bool {
    if address == TIP20_CHANNEL_RESERVE_ADDRESS {
        hardfork.is_t5()
    } else if address == RECEIVE_POLICY_GUARD_ADDRESS {
        hardfork.is_t6()
    } else if address == STORAGE_CREDITS_ADDRESS {
        hardfork.is_t7()
    } else if address == ADDRESS_REGISTRY_ADDRESS || address == SIGNATURE_VERIFIER_ADDRESS {
        hardfork.is_t3()
    } else {
        true
    }
}

fn active_tempo_precompiles(
    hardfork: Option<TempoHardfork>,
) -> impl Iterator<Item = (&'static str, Address)> {
    TEMPO_PRECOMPILES.iter().copied().filter(move |(_, address)| {
        hardfork.is_none_or(|hardfork| is_tempo_precompile_active_at(*address, hardfork))
    })
}

#[cfg(feature = "hashkey")]
#[allow(dead_code)]
fn hashkey_b20_type_identity_probe(
    precompiles: &mut PrecompilesMap,
    activation_admin: Option<Address>,
) {
    let _config = B20Config::DISABLED;
    B20Factory::install_with_observer(precompiles, B20Spec::Beryl, NoopPrecompileCallObserver);
    PolicyRegistryPrecompile::install(precompiles, B20Spec::Beryl);
    ActivationRegistry::install(precompiles, activation_admin);
    BerylLookup::install(precompiles);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum NetworkVariant {
    #[default]
    Ethereum,
    Optimism,
    Tempo,
    #[cfg(feature = "hashkey")]
    HashKey,
}

impl NetworkVariant {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ethereum => "ethereum",
            Self::Optimism => "optimism",
            Self::Tempo => "tempo",
            #[cfg(feature = "hashkey")]
            Self::HashKey => "hashkey",
        }
    }
}

impl std::fmt::Display for NetworkVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl From<ChainId> for NetworkVariant {
    fn from(chain_id: ChainId) -> Self {
        let chain = Chain::from_id(chain_id);
        if chain.is_tempo() {
            Self::Tempo
        } else if chain.is_optimism() {
            Self::Optimism
        } else {
            Self::Ethereum
        }
    }
}

/// The base EVM semantics selected for a resolved network profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvmFamily {
    /// Canonical Ethereum execution semantics.
    #[default]
    Ethereum,
    /// OP Stack execution semantics.
    Optimism,
    /// Tempo execution semantics.
    Tempo,
}

impl EvmFamily {
    /// Returns the family name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ethereum => "ethereum",
            Self::Optimism => "optimism",
            Self::Tempo => "tempo",
        }
    }
}

/// The minimum runtime facts needed to project network-specific EVM semantics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkExecutionContext {
    /// Chain ID of the executing EVM.
    pub chain_id: ChainId,
    /// Timestamp fixed when the EVM is created.
    pub timestamp: u64,
}

impl NetworkExecutionContext {
    /// Creates a new execution context.
    pub const fn new(chain_id: ChainId, timestamp: u64) -> Self {
        Self { chain_id, timestamp }
    }
}

/// State preparation selected by a resolved network profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkStatePlan {
    /// No profile-owned state preparation is required.
    #[default]
    None,
    /// Apply the existing Tempo state preparation path.
    Tempo,
    /// Apply the HashKey B20 development genesis state.
    #[cfg(feature = "hashkey")]
    HashKey,
}

/// Error returned when two network extensions claim the same singleton precompile address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompileCompositionError {
    profile: &'static str,
    address: Address,
    existing: PrecompileId,
    requested: Option<PrecompileId>,
}

impl PrecompileCompositionError {
    /// Returns the profile that failed composition.
    pub const fn profile(&self) -> &'static str {
        self.profile
    }

    /// Returns the conflicting singleton address.
    pub const fn address(&self) -> Address {
        self.address
    }

    /// Returns the precompile already installed at the address.
    pub const fn existing(&self) -> &PrecompileId {
        &self.existing
    }

    /// Returns the precompile the profile requested, or `None` for removal.
    pub const fn requested(&self) -> Option<&PrecompileId> {
        self.requested.as_ref()
    }
}

impl std::fmt::Display for PrecompileCompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "network profile `{}` cannot compose precompile at {}: existing `{}` conflicts with ",
            self.profile,
            self.address,
            self.existing.name(),
        )?;
        if let Some(requested) = &self.requested {
            write!(f, "`{}`", requested.name())
        } else {
            f.write_str("removal")
        }
    }
}

impl std::error::Error for PrecompileCompositionError {}

/// Immutable runtime network semantics resolved from user configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolvedNetworkProfile {
    family: EvmFamily,
    celo: bool,
    bypass_prevrandao: bool,
    #[cfg(feature = "hashkey")]
    hashkey: bool,
    #[cfg(feature = "hashkey")]
    b20_activation_time: Option<u64>,
    #[cfg(feature = "hashkey")]
    b20_activation_admin: Option<Address>,
}

impl ResolvedNetworkProfile {
    /// Returns the selected EVM family.
    pub const fn evm_family(self) -> EvmFamily {
        self.family
    }

    /// Returns the resolved profile name.
    pub const fn name(self) -> &'static str {
        #[cfg(feature = "hashkey")]
        if self.hashkey {
            return "hashkey";
        }
        if self.celo { "celo" } else { self.family.name() }
    }

    /// Returns whether the Celo extension is enabled.
    pub const fn is_celo(self) -> bool {
        self.celo
    }

    /// Returns whether Tempo semantics are selected.
    pub const fn is_tempo(self) -> bool {
        matches!(self.family, EvmFamily::Tempo)
    }

    /// Returns whether Optimism semantics are selected.
    pub const fn is_optimism(self) -> bool {
        matches!(self.family, EvmFamily::Optimism)
    }

    /// Returns whether the HashKey B20 extension is enabled.
    #[cfg(feature = "hashkey")]
    pub const fn is_hashkey(self) -> bool {
        self.hashkey
    }

    /// Returns the B20 consensus configuration for standalone local development.
    #[cfg(feature = "hashkey")]
    pub fn b20_config(self) -> B20Config {
        B20Config::new(self.b20_activation_time, self.b20_activation_admin)
            .expect("resolved HashKey B20 config is valid")
    }

    /// Returns the state preparation plan for this profile.
    pub const fn state_plan(self) -> NetworkStatePlan {
        #[cfg(feature = "hashkey")]
        if self.hashkey {
            return NetworkStatePlan::HashKey;
        }
        if self.is_tempo() { NetworkStatePlan::Tempo } else { NetworkStatePlan::None }
    }

    /// Returns the base fee parameters for this profile.
    pub fn base_fee_params(self, timestamp: u64) -> BaseFeeParams {
        if self.is_optimism() {
            let op_hardforks = OpChainHardforks::op_mainnet();
            if op_hardforks.is_canyon_active_at_timestamp(timestamp) {
                return BaseFeeParams::optimism_canyon();
            }
            return BaseFeeParams::optimism();
        }
        BaseFeeParams::ethereum()
    }

    /// Returns whether prevrandao should be bypassed for the executing chain.
    pub fn bypass_prevrandao(self, chain_id: u64) -> bool {
        if let Ok(
            Moonbeam | Moonbase | Moonriver | MoonbeamDev | Rsk | RskTestnet | Gnosis | Chiado,
        ) = NamedChain::try_from(chain_id)
        {
            return true;
        }
        self.bypass_prevrandao
    }

    /// Injects precompiles projected by this profile.
    pub fn inject_precompiles(
        self,
        precompiles: &mut PrecompilesMap,
        _context: NetworkExecutionContext,
    ) -> Result<(), PrecompileCompositionError> {
        #[cfg(feature = "hashkey")]
        if self.hashkey {
            self.inject_b20_precompiles(precompiles, _context)?;
        }
        if self.celo {
            precompiles.apply_precompile(&CELO_TRANSFER_ADDRESS, move |_| {
                Some(celo::transfer::precompile())
            });
        }
        Ok(())
    }

    /// Installs B20 singletons and dynamic lookup when the activation snapshot is active.
    #[cfg(feature = "hashkey")]
    fn inject_b20_precompiles(
        self,
        precompiles: &mut PrecompilesMap,
        context: NetworkExecutionContext,
    ) -> Result<(), PrecompileCompositionError> {
        let config = self.b20_config();
        if !config.is_active_at(context.timestamp) {
            return Ok(());
        }

        self.ensure_b20_singleton_free(precompiles, B20_FACTORY)?;
        self.ensure_b20_singleton_free(precompiles, B20_ACTIVATION_REGISTRY)?;
        self.ensure_b20_singleton_free(precompiles, B20_POLICY_REGISTRY)?;

        B20Factory::install_with_observer(precompiles, B20Spec::Beryl, NoopPrecompileCallObserver);
        PolicyRegistryPrecompile::install(precompiles, B20Spec::Beryl);
        ActivationRegistry::install(precompiles, config.activation_admin());
        precompiles.map_precompile_lookup(|address, previous| {
            BerylLookup::lookup(address)
                .or_else(|| previous.and_then(|lookup| lookup.lookup(address)))
        });

        Ok(())
    }

    #[cfg(feature = "hashkey")]
    fn ensure_b20_singleton_free(
        self,
        precompiles: &PrecompilesMap,
        address: Address,
    ) -> Result<(), PrecompileCompositionError> {
        if let Some(existing) = precompiles.get(&address) {
            return Err(PrecompileCompositionError {
                profile: self.name(),
                address,
                existing: existing.precompile_id().clone(),
                requested: None,
            });
        }
        Ok(())
    }

    /// Returns trace labels projected by this profile.
    pub fn precompile_labels(
        self,
        tempo_hardfork: Option<TempoHardfork>,
    ) -> AddressHashMap<String> {
        let mut labels = AddressHashMap::default();
        if self.celo {
            labels.insert(CELO_TRANSFER_ADDRESS, CELO_TRANSFER_LABEL.to_string());
        }
        #[cfg(feature = "hashkey")]
        if self.hashkey {
            labels.insert(B20_FACTORY, "B20Factory".to_string());
            labels.insert(B20_ACTIVATION_REGISTRY, "B20ActivationRegistry".to_string());
            labels.insert(B20_POLICY_REGISTRY, "B20PolicyRegistry".to_string());
        }
        if self.is_tempo() {
            labels.extend(
                active_tempo_precompiles(tempo_hardfork)
                    .map(|(label, address)| (address, label.to_string())),
            );
        }
        labels
    }

    /// Returns the static precompile inventory projected by this profile.
    pub fn precompile_inventory(
        self,
        tempo_hardfork: Option<TempoHardfork>,
    ) -> BTreeMap<String, Address> {
        let mut precompiles = BTreeMap::new();
        if self.celo {
            precompiles
                .insert(PRECOMPILE_ID_CELO_TRANSFER.name().to_string(), CELO_TRANSFER_ADDRESS);
        }
        #[cfg(feature = "hashkey")]
        if self.hashkey {
            precompiles.insert("B20Factory".to_string(), B20_FACTORY);
            precompiles.insert("B20ActivationRegistry".to_string(), B20_ACTIVATION_REGISTRY);
            precompiles.insert("B20PolicyRegistry".to_string(), B20_POLICY_REGISTRY);
        }
        if self.is_tempo() {
            precompiles.extend(
                active_tempo_precompiles(tempo_hardfork)
                    .map(|(label, address)| (label.to_string(), address)),
            );
        }
        precompiles
    }
}

#[derive(Clone, Debug, Default, Parser, Deserialize, Copy, PartialEq, Eq)]
pub struct NetworkConfigs {
    /// Enable a specific network profile.
    #[arg(help_heading = "Networks", long, short, num_args = 1, value_name = "NETWORK", value_enum, conflicts_with_all = ["celo", "optimism", "tempo"])]
    #[serde(default)]
    network: Option<NetworkVariant>,
    /// Enable Celo network features.
    #[arg(help_heading = "Networks", long, conflicts_with_all = ["network", "optimism", "tempo"])]
    celo: bool,
    /// Enable Optimism network features (deprecated: use --network optimism).
    #[arg(long, hide = true, conflicts_with_all = ["network", "celo", "tempo"])]
    // Deserialize-only legacy alias: accepted in foundry.toml but never serialized — the
    // canonical form is `network = "optimism"`.
    #[serde(default)]
    optimism: bool,
    /// Enable Tempo network features (deprecated: use --network tempo).
    #[arg(long, hide = true, conflicts_with_all = ["network", "celo", "optimism"])]
    // Deserialize-only legacy alias: accepted in foundry.toml but never serialized — the
    // canonical form is `network = "tempo"`.
    #[serde(default)]
    tempo: bool,
    /// Whether to bypass prevrandao.
    #[arg(skip)]
    #[serde(default)]
    bypass_prevrandao: bool,
}

// Custom `Serialize` impl: always emits the *resolved* network as the canonical
// `network = "..."` field, and never emits the legacy `tempo` / `optimism` aliases. This avoids
// confusing output like `network = "tempo"` next to `tempo = false`, and ensures `tempo = true`
// in foundry.toml round-trips as `network = "tempo"`.
impl Serialize for NetworkConfigs {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("NetworkConfigs", 3)?;
        s.serialize_field("network", &self.resolved_network())?;
        s.serialize_field("celo", &self.celo)?;
        s.serialize_field("bypass_prevrandao", &self.bypass_prevrandao)?;
        s.end()
    }
}

impl NetworkConfigs {
    pub fn with_optimism() -> Self {
        Self { network: Some(NetworkVariant::Optimism), optimism: true, ..Default::default() }
    }

    pub fn with_celo() -> Self {
        Self { celo: true, ..Default::default() }
    }

    pub fn with_tempo() -> Self {
        Self { network: Some(NetworkVariant::Tempo), tempo: true, ..Default::default() }
    }

    /// Selects the HashKey B20 network profile.
    #[cfg(feature = "hashkey")]
    pub fn with_hashkey() -> Self {
        Self { network: Some(NetworkVariant::HashKey), ..Default::default() }
    }

    pub const fn is_optimism(&self) -> bool {
        (*self).resolve().is_optimism()
    }

    pub const fn is_tempo(&self) -> bool {
        (*self).resolve().is_tempo()
    }

    pub const fn is_celo(&self) -> bool {
        self.celo
    }

    /// Resolves user configuration into immutable runtime network semantics.
    pub const fn resolve(self) -> ResolvedNetworkProfile {
        let network = self.resolved_network();
        #[cfg(feature = "hashkey")]
        let hashkey = matches!(network, Some(NetworkVariant::HashKey));
        let family = match network {
            None | Some(NetworkVariant::Ethereum) => EvmFamily::Ethereum,
            Some(NetworkVariant::Optimism) => EvmFamily::Optimism,
            Some(NetworkVariant::Tempo) => EvmFamily::Tempo,
            #[cfg(feature = "hashkey")]
            Some(NetworkVariant::HashKey) => EvmFamily::Optimism,
        };
        ResolvedNetworkProfile {
            family,
            celo: self.celo,
            bypass_prevrandao: self.bypass_prevrandao,
            #[cfg(feature = "hashkey")]
            hashkey,
            #[cfg(feature = "hashkey")]
            b20_activation_time: if hashkey { Some(0) } else { None },
            #[cfg(feature = "hashkey")]
            b20_activation_admin: if hashkey { Some(HSK_B20_LOCAL_ADMIN) } else { None },
        }
    }

    /// Returns the resolved network variant, folding legacy flags.
    pub const fn resolved_network(&self) -> Option<NetworkVariant> {
        if let Some(network) = self.network {
            return Some(network);
        }
        if self.optimism {
            return Some(NetworkVariant::Optimism);
        }
        if self.tempo {
            return Some(NetworkVariant::Tempo);
        }
        None
    }

    /// Returns the name of the currently active non-Ethereum network, or `None` for plain Ethereum.
    pub fn active_network_name(&self) -> Option<&'static str> {
        self.resolved_network().and_then(|n| match n {
            NetworkVariant::Ethereum => None,
            _ => Some(n.name()),
        })
    }

    /// Returns the base fee parameters for the configured network.
    ///
    /// For Optimism networks, returns Canyon parameters if the Canyon hardfork is active
    /// at the given timestamp, otherwise returns pre-Canyon parameters.
    pub fn base_fee_params(&self, timestamp: u64) -> BaseFeeParams {
        self.resolve().base_fee_params(timestamp)
    }

    pub fn bypass_prevrandao(&self, chain_id: u64) -> bool {
        self.resolve().bypass_prevrandao(chain_id)
    }

    pub fn with_chain_id(self, chain_id: u64) -> Self {
        let chain = Chain::from_id(chain_id);
        if self.resolved_network().is_none() {
            if chain.is_tempo() {
                Self::with_tempo()
            } else if chain.is_optimism() {
                Self::with_optimism()
            } else {
                self
            }
        } else if !self.celo
            && matches!(chain.named(), Some(NamedChain::Celo | NamedChain::CeloSepolia))
        {
            Self::with_celo()
        } else {
            self
        }
    }

    /// Validates `hardfork` against the current `NetworkConfigs` and, if consistent, returns an
    /// updated instance with the network implied by the enabled hardfork.
    ///
    /// Returns `Err` when the hardfork's network family conflicts with the configured one.
    pub fn normalize_for_hardfork(self, hardfork: FoundryHardfork) -> Result<Self, String> {
        let hardfork_namespace = hardfork.namespace();
        if let Some(configured) = self.active_network_name().filter(|&name| {
            #[cfg(feature = "hashkey")]
            if name == "hashkey" && hardfork_namespace == Some("optimism") {
                return false;
            }
            Some(name) != hardfork_namespace
        }) {
            return Err(format!(
                "hardfork `{}` conflicts with network config `{configured}`",
                String::from(hardfork),
            ));
        }

        let network = match hardfork {
            FoundryHardfork::Ethereum(_) => self,
            FoundryHardfork::Tempo(_) => Self::with_tempo(),
            FoundryHardfork::Optimism(_) => {
                #[cfg(feature = "hashkey")]
                if matches!(self.resolved_network(), Some(NetworkVariant::HashKey)) {
                    return Ok(self);
                }
                Self::with_optimism()
            }
        };

        Ok(network)
    }

    /// Inject precompiles for configured networks.
    pub fn inject_precompiles(self, precompiles: &mut PrecompilesMap) {
        self.resolve()
            .inject_precompiles(precompiles, NetworkExecutionContext::default())
            .expect("legacy network precompile composition is infallible");
    }

    /// Returns precompiles label for configured networks, to be used in traces.
    pub fn precompiles_label(self) -> AddressHashMap<String> {
        let mut labels = AddressHashMap::default();
        if self.celo {
            labels.insert(CELO_TRANSFER_ADDRESS, CELO_TRANSFER_LABEL.to_string());
        }
        labels
    }

    /// Returns precompiles for configured networks.
    pub fn precompiles(self) -> BTreeMap<String, Address> {
        let mut precompiles = BTreeMap::new();
        if self.celo {
            precompiles
                .insert(PRECOMPILE_ID_CELO_TRANSFER.name().to_string(), CELO_TRANSFER_ADDRESS);
        }
        precompiles
    }
}

impl From<NetworkVariant> for NetworkConfigs {
    fn from(network: NetworkVariant) -> Self {
        match network {
            NetworkVariant::Ethereum => Self::default(),
            NetworkVariant::Optimism => {
                Self { network: Some(network), optimism: true, ..Default::default() }
            }
            NetworkVariant::Tempo => {
                Self { network: Some(network), tempo: true, ..Default::default() }
            }
            #[cfg(feature = "hashkey")]
            NetworkVariant::HashKey => Self { network: Some(network), ..Default::default() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::precompile::Precompiles;

    #[cfg(feature = "hashkey")]
    #[test]
    fn hashkey_b20_packages_share_precompiles_map_type() {
        let mut precompiles = PrecompilesMap::from_static(Precompiles::cancun());
        hashkey_b20_type_identity_probe(&mut precompiles, None);
    }

    #[test]
    fn resolves_configuration_into_runtime_profile() {
        let ethereum = NetworkConfigs::default().resolve();
        assert_eq!(ethereum.evm_family(), EvmFamily::Ethereum);
        assert_eq!(ethereum.name(), "ethereum");
        assert_eq!(ethereum.state_plan(), NetworkStatePlan::None);

        let celo = NetworkConfigs::with_celo().resolve();
        assert_eq!(celo.evm_family(), EvmFamily::Ethereum);
        assert_eq!(celo.name(), "celo");
        assert!(celo.is_celo());
        assert_eq!(
            celo.precompile_inventory(None).get(PRECOMPILE_ID_CELO_TRANSFER.name()),
            Some(&CELO_TRANSFER_ADDRESS)
        );
        assert_eq!(
            celo.precompile_labels(None).get(&CELO_TRANSFER_ADDRESS),
            Some(&CELO_TRANSFER_LABEL.to_string())
        );

        let tempo = NetworkConfigs::with_tempo().resolve();
        assert_eq!(tempo.evm_family(), EvmFamily::Tempo);
        assert_eq!(tempo.name(), "tempo");
        assert_eq!(tempo.state_plan(), NetworkStatePlan::Tempo);
        let tip20_factory = alloy_primitives::address!("20FC000000000000000000000000000000000000");
        assert_eq!(tempo.precompile_inventory(None).get("TIP20Factory"), Some(&tip20_factory));
        assert_eq!(
            tempo.precompile_labels(None).get(&tip20_factory),
            Some(&"TIP20Factory".to_string())
        );

        let optimism = NetworkConfigs::with_optimism().resolve();
        assert_eq!(optimism.evm_family(), EvmFamily::Optimism);
        assert_eq!(optimism.name(), "optimism");
    }

    #[test]
    fn profile_precompile_projection_preserves_celo_behavior() {
        let mut precompiles = PrecompilesMap::from_static(Precompiles::cancun());
        NetworkConfigs::with_celo()
            .resolve()
            .inject_precompiles(&mut precompiles, NetworkExecutionContext::new(1, 0))
            .unwrap();

        assert!(precompiles.get(&CELO_TRANSFER_ADDRESS).is_some());
    }

    #[test]
    fn legacy_tempo_projection_entry_points_remain_unchanged() {
        let configs = NetworkConfigs::with_tempo();
        assert!(configs.precompiles_label().is_empty());
        assert!(configs.precompiles().is_empty());
    }

    #[test]
    fn tempo_profile_filters_precompile_projection_by_hardfork() {
        let profile = NetworkConfigs::with_tempo().resolve();

        assert!(
            !profile.precompile_inventory(Some(TempoHardfork::T2)).contains_key("AddressRegistry")
        );
        assert!(
            profile.precompile_inventory(Some(TempoHardfork::T3)).contains_key("AddressRegistry")
        );
        assert!(
            !profile
                .precompile_inventory(Some(TempoHardfork::T4))
                .contains_key("TIP20ChannelReserve")
        );
        assert!(
            profile
                .precompile_inventory(Some(TempoHardfork::T5))
                .contains_key("TIP20ChannelReserve")
        );
        assert!(
            !profile
                .precompile_labels(Some(TempoHardfork::T6))
                .contains_key(&STORAGE_CREDITS_ADDRESS)
        );
        assert!(
            profile
                .precompile_labels(Some(TempoHardfork::T7))
                .contains_key(&STORAGE_CREDITS_ADDRESS)
        );
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn hashkey_selector_resolves_optimism_b20_profile() {
        let configs = NetworkConfigs::parse_from(["test", "--network", "hashkey"]);
        let profile = configs.resolve();

        assert_eq!(profile.evm_family(), EvmFamily::Optimism);
        assert_eq!(profile.name(), "hashkey");
        assert!(profile.is_optimism());
        assert!(profile.is_hashkey());
        assert_eq!(profile.state_plan(), NetworkStatePlan::HashKey);

        let b20_config = profile.b20_config();
        assert_eq!(b20_config.activation_time(), Some(0));
        assert_eq!(b20_config.activation_admin(), Some(HSK_B20_LOCAL_ADMIN));
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn optimism_hardfork_preserves_hashkey_selector() {
        let configs = NetworkConfigs::with_hashkey()
            .normalize_for_hardfork(FoundryHardfork::Optimism(
                alloy_op_hardforks::OpHardfork::Bedrock,
            ))
            .unwrap();

        assert!(configs.resolve().is_hashkey());
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn hashkey_capability_requires_runtime_selection() {
        const B20_FACTORY: Address =
            alloy_primitives::address!("B20F000000000000000000000000000000000000");

        let ethereum = NetworkConfigs::default().resolve();
        assert!(!ethereum.is_hashkey());
        assert_eq!(ethereum.b20_config().activation_time(), None);
        assert_eq!(ethereum.b20_config().activation_admin(), None);

        let optimism = NetworkConfigs::with_optimism().resolve();
        assert!(!optimism.is_hashkey());
        assert_eq!(optimism.b20_config().activation_time(), None);
        assert_eq!(optimism.b20_config().activation_admin(), None);
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());
        optimism
            .inject_precompiles(&mut precompiles, NetworkExecutionContext::new(177, 0))
            .unwrap();
        assert!(precompiles.get(&B20_FACTORY).is_none());

        let configs = NetworkConfigs::with_hashkey();
        assert!(configs.is_optimism());
        assert!(configs.resolve().is_hashkey());
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn hashkey_selector_roundtrips_as_canonical_network() {
        let json = serde_json::to_value(NetworkConfigs::with_hashkey()).unwrap();
        assert_eq!(json["network"], serde_json::json!("hashkey"));

        let restored = serde_json::from_value::<NetworkConfigs>(json).unwrap();
        assert!(restored.resolve().is_hashkey());
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn hashkey_profile_projects_active_b20_precompiles() {
        const B20_FACTORY: Address =
            alloy_primitives::address!("B20F000000000000000000000000000000000000");
        const B20_ACTIVATION_REGISTRY: Address =
            alloy_primitives::address!("8453000000000000000000000000000000000001");
        const B20_POLICY_REGISTRY: Address =
            alloy_primitives::address!("8453000000000000000000000000000000000002");

        let profile = NetworkConfigs::with_hashkey().resolve();
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());
        profile.inject_precompiles(&mut precompiles, NetworkExecutionContext::new(177, 0)).unwrap();

        assert!(precompiles.get(&B20_FACTORY).is_some());
        assert!(precompiles.get(&B20_ACTIVATION_REGISTRY).is_some());
        assert!(precompiles.get(&B20_POLICY_REGISTRY).is_some());

        let labels = profile.precompile_labels(None);
        assert_eq!(labels.get(&B20_FACTORY), Some(&"B20Factory".to_string()));
        assert_eq!(
            labels.get(&B20_ACTIVATION_REGISTRY),
            Some(&"B20ActivationRegistry".to_string())
        );
        assert_eq!(labels.get(&B20_POLICY_REGISTRY), Some(&"B20PolicyRegistry".to_string()));

        let inventory = profile.precompile_inventory(None);
        assert_eq!(inventory.get("B20Factory"), Some(&B20_FACTORY));
        assert_eq!(inventory.get("B20ActivationRegistry"), Some(&B20_ACTIVATION_REGISTRY));
        assert_eq!(inventory.get("B20PolicyRegistry"), Some(&B20_POLICY_REGISTRY));
    }

    #[cfg(feature = "hashkey")]
    #[test]
    fn hashkey_profile_rejects_singleton_collision() {
        const B20_FACTORY: Address =
            alloy_primitives::address!("B20F000000000000000000000000000000000000");

        let conflicting_id = PrecompileId::Custom(std::borrow::Cow::Borrowed("conflict-test"));
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());
        precompiles.apply_precompile(&B20_FACTORY, {
            let conflicting_id = conflicting_id.clone();
            move |_| {
                Some(alloy_evm::precompiles::DynPrecompile::new(conflicting_id, |_| {
                    unreachable!("not executed")
                }))
            }
        });

        let err = NetworkConfigs::with_hashkey()
            .resolve()
            .inject_precompiles(&mut precompiles, NetworkExecutionContext::new(177, 0))
            .unwrap_err();

        assert_eq!(err.profile(), "hashkey");
        assert_eq!(err.address(), B20_FACTORY);
        assert_eq!(err.existing(), &conflicting_id);
        assert_eq!(err.requested(), None);
    }

    // --- Equivalence: new flag == legacy flag ---

    #[test]
    fn new_tempo_flag_equivalent_to_legacy() {
        let via_new = NetworkConfigs { network: Some(NetworkVariant::Tempo), ..Default::default() };
        let via_old = NetworkConfigs { tempo: true, ..Default::default() };
        assert_eq!(via_new.is_tempo(), via_old.is_tempo());
        assert_eq!(via_new.is_optimism(), via_old.is_optimism());
        assert_eq!(via_new.active_network_name(), via_old.active_network_name());
    }

    #[test]
    fn new_optimism_flag_equivalent_to_legacy() {
        let via_new =
            NetworkConfigs { network: Some(NetworkVariant::Optimism), ..Default::default() };
        let via_old = NetworkConfigs { optimism: true, ..Default::default() };
        assert_eq!(via_new.is_optimism(), via_old.is_optimism());
        assert_eq!(via_new.is_tempo(), via_old.is_tempo());
        assert_eq!(via_new.active_network_name(), via_old.active_network_name());
    }

    // --- resolved() / active_network_name ---

    #[test]
    fn active_network_name_tempo() {
        let cfg = NetworkConfigs::with_tempo();
        assert_eq!(cfg.active_network_name(), Some("tempo"));
    }

    #[test]
    fn active_network_name_optimism() {
        let cfg = NetworkConfigs::with_optimism();
        assert_eq!(cfg.active_network_name(), Some("optimism"));
    }

    #[test]
    fn active_network_name_default_is_none() {
        assert_eq!(NetworkConfigs::default().active_network_name(), None);
    }

    // --- new flag takes precedence over legacy flag ---

    #[test]
    fn new_flag_wins_over_legacy_when_both_set() {
        // --network optimism --tempo: network field wins
        let cfg = NetworkConfigs {
            network: Some(NetworkVariant::Optimism),
            tempo: true,
            ..Default::default()
        };
        assert!(cfg.is_optimism());
        assert!(!cfg.is_tempo());
    }

    // --- Serde round-trip ---

    #[test]
    fn serde_roundtrip_tempo() {
        let original = NetworkConfigs::with_tempo();
        let json = serde_json::to_string(&original).unwrap();
        let restored: NetworkConfigs = serde_json::from_str(&json).unwrap();
        assert!(restored.is_tempo());
        assert!(!restored.is_optimism());
    }

    #[test]
    fn serde_roundtrip_optimism() {
        let original = NetworkConfigs::with_optimism();
        let json = serde_json::to_string(&original).unwrap();
        let restored: NetworkConfigs = serde_json::from_str(&json).unwrap();
        assert!(restored.is_optimism());
        assert!(!restored.is_tempo());
    }

    #[test]
    fn serde_legacy_tempo_bool_deserialized() {
        // Old foundry.toml format: `tempo = true`
        let json = r#"{"tempo": true, "celo": false, "bypass_prevrandao": false}"#;
        let cfg: NetworkConfigs = serde_json::from_str(json).unwrap();
        assert!(cfg.is_tempo());
    }

    #[test]
    fn serde_serializes_legacy_alias_as_canonical_network() {
        // Legacy `tempo = true` should serialize as the canonical `network = "tempo"`,
        // and the legacy `tempo` / `optimism` keys must not appear in the output.
        let cfg = NetworkConfigs { tempo: true, ..Default::default() };
        let json = serde_json::to_value(cfg).unwrap();
        assert_eq!(json["network"], serde_json::json!("tempo"));
        assert!(json.get("tempo").is_none(), "legacy `tempo` key should not be serialized");
        assert!(json.get("optimism").is_none(), "legacy `optimism` key should not be serialized");
    }

    #[test]
    fn serde_new_network_field_deserialized() {
        let json_tempo = r#"{"network": "tempo", "celo": false, "bypass_prevrandao": false}"#;
        let cfg_tempo: NetworkConfigs = serde_json::from_str(json_tempo).unwrap();
        assert!(cfg_tempo.is_tempo());
        let json_optimism = r#"{"network": "optimism", "celo": false, "bypass_prevrandao": false}"#;
        let cfg_optimism: NetworkConfigs = serde_json::from_str(json_optimism).unwrap();
        assert!(cfg_optimism.is_optimism());
    }
}
