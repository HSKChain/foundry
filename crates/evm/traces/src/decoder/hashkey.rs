//! HashKey B20 trace decoding backed by the pinned canonical interfaces.

use super::indexed_inputs;
use alloy_json_abi::{Event, Function, JsonAbi};
use alloy_primitives::{B256, Selector, map::HashMap};
use foundry_evm_core::decode::RevertDecoder;
use foundry_evm_networks::NetworkTraceIdentity;
use std::{collections::BTreeMap, sync::OnceLock};

const ACTIVATION_ABI: &[&str] = &[
    "event FeatureActivated(bytes32 indexed feature,address indexed caller)",
    "event FeatureDeactivated(bytes32 indexed feature,address indexed caller)",
    "error Unauthorized(address caller)",
    "error AlreadyActivated(bytes32 feature)",
    "error FeatureNotActivated(bytes32 feature)",
    "error DelegateCallNotAllowed()",
    "error StaticCallNotAllowed()",
    "function isActivated(bytes32 feature) view returns (bool)",
    "function checkActivated(bytes32 feature) view",
    "function admin() view returns (address)",
    "function activate(bytes32 feature)",
    "function deactivate(bytes32 feature)",
];

const POLICY_ABI: &[&str] = &[
    "error NonPayable()",
    "error Unauthorized()",
    "error PolicyNotFound()",
    "error IncompatiblePolicyType()",
    "error ZeroAddress()",
    "error BatchSizeTooLarge(uint256 maxBatchSize)",
    "error NoPendingAdmin()",
    "event PolicyCreated(uint64 indexed policyId,address indexed creator,uint8 policyType)",
    "event PolicyAdminStaged(uint64 indexed policyId,address indexed currentAdmin,address indexed pendingAdmin)",
    "event PolicyAdminUpdated(uint64 indexed policyId,address indexed previousAdmin,address indexed newAdmin)",
    "event AllowlistUpdated(uint64 indexed policyId,address indexed updater,bool allowed,address[] accounts)",
    "event BlocklistUpdated(uint64 indexed policyId,address indexed updater,bool blocked,address[] accounts)",
    "function createPolicy(address admin,uint8 policyType) returns (uint64)",
    "function createPolicyWithAccounts(address admin,uint8 policyType,address[] accounts) returns (uint64)",
    "function stageUpdateAdmin(uint64 policyId,address newAdmin)",
    "function finalizeUpdateAdmin(uint64 policyId)",
    "function renounceAdmin(uint64 policyId)",
    "function updateAllowlist(uint64 policyId,bool allowed,address[] accounts)",
    "function updateBlocklist(uint64 policyId,bool blocked,address[] accounts)",
    "function isAuthorized(uint64 policyId,address account) view returns (bool)",
    "function policyExists(uint64 policyId) view returns (bool)",
    "function policyAdmin(uint64 policyId) view returns (address)",
    "function pendingPolicyAdmin(uint64 policyId) view returns (address)",
];

const FACTORY_ABI: &[&str] = &[
    "error NonPayable()",
    "error TokenAlreadyExists(address token)",
    "error InvalidVariant()",
    "error UnsupportedVersion(uint8 version,uint8 variant)",
    "error MissingRequiredField(string field)",
    "error InvalidCurrency(string code)",
    "error InvalidDecimals(uint8 decimals)",
    "error InitCallFailed(uint256 index)",
    "event B20Created(address indexed token,uint8 indexed variant,string name,string symbol,uint8 decimals,bytes variantParams)",
    "function createB20(uint8 variant,bytes32 salt,bytes params,bytes[] initCalls) returns (address token)",
    "function getB20Address(uint8 variant,address sender,bytes32 salt) view returns (address)",
    "function isB20(address token) view returns (bool)",
    "function isB20Initialized(address token) view returns (bool)",
];

const COMMON_ABI: &[&str] = &[
    "error NonPayable()",
    "error AccessControlUnauthorizedAccount(address account,bytes32 neededRole)",
    "error Unauthorized()",
    "error ContractPaused(uint8 feature)",
    "error InsufficientAllowance(address spender,uint256 allowance,uint256 needed)",
    "error InsufficientBalance(address sender,uint256 balance,uint256 needed)",
    "error InvalidSender(address sender)",
    "error InvalidReceiver(address receiver)",
    "error InvalidApprover(address approver)",
    "error InvalidSpender(address spender)",
    "error InvalidAmount()",
    "error EmptyFeatureSet()",
    "error InvalidSupplyCap(uint256 currentSupply,uint256 proposedCap)",
    "error SupplyCapExceeded(uint256 cap,uint256 attempted)",
    "error PolicyForbids(bytes32 policyScope,uint64 policyId)",
    "error PolicyNotFound(uint64 policyId)",
    "error UnsupportedPolicyType(bytes32 policyScope)",
    "error AccountNotBlocked(address account)",
    "error ExpiredSignature(uint256 deadline)",
    "error InvalidSigner(address signer,address owner)",
    "error LastAdminCannotRenounce()",
    "error NotSoleAdmin()",
    "error AccessControlBadConfirmation()",
    "event Transfer(address indexed from,address indexed to,uint256 amount)",
    "event Approval(address indexed owner,address indexed spender,uint256 amount)",
    "event Memo(address indexed caller,bytes32 indexed memo)",
    "event BurnedBlocked(address indexed caller,address indexed from,uint256 amount)",
    "event RoleGranted(bytes32 indexed role,address indexed account,address indexed sender)",
    "event RoleRevoked(bytes32 indexed role,address indexed account,address indexed sender)",
    "event RoleAdminChanged(bytes32 indexed role,bytes32 indexed previousAdminRole,bytes32 indexed newAdminRole)",
    "event LastAdminRenounced(address indexed previousAdmin)",
    "event Paused(address indexed updater,uint8[] features)",
    "event Unpaused(address indexed updater,uint8[] features)",
    "event PolicyUpdated(bytes32 indexed policyScope,uint64 oldPolicyId,uint64 newPolicyId)",
    "event SupplyCapUpdated(address indexed updater,uint256 oldSupplyCap,uint256 newSupplyCap)",
    "event ContractURIUpdated()",
    "event NameUpdated(address indexed updater,string newName)",
    "event SymbolUpdated(address indexed updater,string newSymbol)",
    "event EIP712DomainChanged()",
    "function DEFAULT_ADMIN_ROLE() view returns (bytes32)",
    "function MINT_ROLE() view returns (bytes32)",
    "function BURN_ROLE() view returns (bytes32)",
    "function BURN_BLOCKED_ROLE() view returns (bytes32)",
    "function PAUSE_ROLE() view returns (bytes32)",
    "function UNPAUSE_ROLE() view returns (bytes32)",
    "function METADATA_ROLE() view returns (bytes32)",
    "function TRANSFER_SENDER_POLICY() view returns (bytes32)",
    "function TRANSFER_RECEIVER_POLICY() view returns (bytes32)",
    "function TRANSFER_EXECUTOR_POLICY() view returns (bytes32)",
    "function MINT_RECEIVER_POLICY() view returns (bytes32)",
    "function name() view returns (string)",
    "function symbol() view returns (string)",
    "function decimals() view returns (uint8)",
    "function totalSupply() view returns (uint256)",
    "function balanceOf(address account) view returns (uint256)",
    "function allowance(address owner,address spender) view returns (uint256)",
    "function transfer(address to,uint256 amount) returns (bool)",
    "function transferFrom(address from,address to,uint256 amount) returns (bool)",
    "function approve(address spender,uint256 amount) returns (bool)",
    "function updateName(string newName)",
    "function updateSymbol(string newSymbol)",
    "function transferWithMemo(address to,uint256 amount,bytes32 memo) returns (bool)",
    "function transferFromWithMemo(address from,address to,uint256 amount,bytes32 memo) returns (bool)",
    "function mint(address to,uint256 amount)",
    "function mintWithMemo(address to,uint256 amount,bytes32 memo)",
    "function burn(uint256 amount)",
    "function burnWithMemo(uint256 amount,bytes32 memo)",
    "function burnBlocked(address from,uint256 amount)",
    "function hasRole(bytes32 role,address account) view returns (bool)",
    "function getRoleAdmin(bytes32 role) view returns (bytes32)",
    "function grantRole(bytes32 role,address account)",
    "function revokeRole(bytes32 role,address account)",
    "function renounceRole(bytes32 role,address callerConfirmation)",
    "function renounceLastAdmin()",
    "function setRoleAdmin(bytes32 role,bytes32 newAdminRole)",
    "function pausedFeatures() view returns (uint8[])",
    "function isPaused(uint8 feature) view returns (bool)",
    "function pause(uint8[] features)",
    "function unpause(uint8[] features)",
    "function policyId(bytes32 policyScope) view returns (uint64)",
    "function updatePolicy(bytes32 policyScope,uint64 newPolicyId)",
    "function supplyCap() view returns (uint256)",
    "function updateSupplyCap(uint256 newSupplyCap)",
    "function DOMAIN_SEPARATOR() view returns (bytes32)",
    "function nonces(address owner) view returns (uint256)",
    "function permit(address owner,address spender,uint256 value,uint256 deadline,uint8 v,bytes32 r,bytes32 s)",
    "function eip712Domain() view returns (bytes1 fields,string name,string version,uint256 chainId,address verifyingContract,bytes32 salt,uint256[] extensions)",
    "function contractURI() view returns (string)",
    "function updateContractURI(string newURI)",
];

const ASSET_ABI: &[&str] = &[
    "error AnnouncementIdAlreadyUsed(string id)",
    "error InvalidMetadataKey()",
    "error InvalidMultiplier()",
    "error LengthMismatch(uint256 leftLen,uint256 rightLen)",
    "error EmptyBatch()",
    "error AnnouncementInProgress()",
    "error InternalCallMalformed(bytes call)",
    "error InternalCallFailed(bytes call)",
    "event MultiplierUpdated(uint256 multiplier)",
    "event ExtraMetadataUpdated(string key,string value)",
    "event Announcement(address indexed caller,string id,string description,string uri)",
    "event EndAnnouncement(string id)",
    "function OPERATOR_ROLE() view returns (bytes32)",
    "function WAD_PRECISION() view returns (uint256)",
    "function announce(bytes[] internalCalls,string id,string description,string uri)",
    "function isAnnouncementIdUsed(string id) view returns (bool)",
    "function multiplier() view returns (uint256)",
    "function toScaledBalance(uint256 rawBalance) view returns (uint256)",
    "function toRawBalance(uint256 scaledBalance) view returns (uint256 rawBalance)",
    "function scaledBalanceOf(address account) view returns (uint256)",
    "function updateMultiplier(uint256 newMultiplier)",
    "function batchMint(address[] recipients,uint256[] amounts)",
    "function extraMetadata(string key) view returns (string)",
    "function updateExtraMetadata(string key,string value)",
];

const STABLECOIN_ABI: &[&str] = &["function currency() view returns (string)"];

pub(super) struct NetworkAbi {
    pub(super) functions: HashMap<Selector, Vec<Function>>,
    pub(super) events: BTreeMap<(B256, usize), Vec<Event>>,
    pub(super) reverts: RevertDecoder,
}

impl NetworkAbi {
    fn new(abis: &[&JsonAbi]) -> Self {
        let functions = abis.iter().flat_map(|abi| abi.functions()).cloned().fold(
            HashMap::<Selector, Vec<Function>>::default(),
            |mut functions, function| {
                functions.entry(function.selector()).or_default().push(function);
                functions
            },
        );
        let events = abis.iter().flat_map(|abi| abi.events()).cloned().fold(
            BTreeMap::<(B256, usize), Vec<Event>>::new(),
            |mut events, event| {
                events.entry((event.selector(), indexed_inputs(&event))).or_default().push(event);
                events
            },
        );
        let reverts = RevertDecoder::new().with_abis(abis.iter().copied());
        Self { functions, events, reverts }
    }

    fn for_identity(identity: NetworkTraceIdentity) -> Self {
        let parse = |items: &[&str]| {
            JsonAbi::parse(items.iter().copied()).expect("pinned HashKey B20 ABI is valid")
        };
        match identity {
            NetworkTraceIdentity::B20Factory => Self::new(&[&parse(FACTORY_ABI)]),
            NetworkTraceIdentity::B20ActivationRegistry => Self::new(&[&parse(ACTIVATION_ABI)]),
            NetworkTraceIdentity::B20PolicyRegistry => Self::new(&[&parse(POLICY_ABI)]),
            NetworkTraceIdentity::B20Asset => Self::new(&[&parse(COMMON_ABI), &parse(ASSET_ABI)]),
            NetworkTraceIdentity::B20Stablecoin => {
                Self::new(&[&parse(COMMON_ABI), &parse(STABLECOIN_ABI)])
            }
        }
    }
}

pub(super) fn network_abi(identity: NetworkTraceIdentity) -> &'static NetworkAbi {
    static FACTORY: OnceLock<NetworkAbi> = OnceLock::new();
    static ACTIVATION: OnceLock<NetworkAbi> = OnceLock::new();
    static POLICY: OnceLock<NetworkAbi> = OnceLock::new();
    static ASSET: OnceLock<NetworkAbi> = OnceLock::new();
    static STABLECOIN: OnceLock<NetworkAbi> = OnceLock::new();

    let cache = match identity {
        NetworkTraceIdentity::B20Factory => &FACTORY,
        NetworkTraceIdentity::B20ActivationRegistry => &ACTIVATION,
        NetworkTraceIdentity::B20PolicyRegistry => &POLICY,
        NetworkTraceIdentity::B20Asset => &ASSET,
        NetworkTraceIdentity::B20Stablecoin => &STABLECOIN,
    };
    cache.get_or_init(|| NetworkAbi::for_identity(identity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_sol_types::{SolEvent, SolEventInterface, SolInterface, TopicList};
    use hsk_b20_precompiles::{
        IActivationRegistry, IB20, IB20Asset, IB20Factory, IB20Stablecoin, IPolicyRegistry,
    };
    use std::collections::BTreeSet;

    fn interface_selectors<I: SolInterface>() -> BTreeSet<Selector> {
        I::selectors().map(Selector::from).collect()
    }

    fn function_selectors(abi: &JsonAbi) -> BTreeSet<Selector> {
        abi.functions().map(Function::selector).collect()
    }

    fn error_selectors(abi: &JsonAbi) -> BTreeSet<Selector> {
        abi.errors().map(alloy_json_abi::Error::selector).collect()
    }

    fn assert_matches_interfaces<C: SolInterface, E: SolInterface>(items: &[&str]) {
        let abi = JsonAbi::parse(items.iter().copied()).unwrap();
        assert_eq!(function_selectors(&abi), interface_selectors::<C>());
        assert_eq!(error_selectors(&abi), interface_selectors::<E>());
    }

    fn event_key<E: SolEvent>() -> (B256, usize) {
        (E::SIGNATURE_HASH, <E::TopicList as TopicList>::COUNT - usize::from(!E::ANONYMOUS))
    }

    fn event_keys(abi: &JsonAbi) -> BTreeSet<(B256, usize)> {
        abi.events().map(|event| (event.selector(), indexed_inputs(event))).collect()
    }

    fn assert_events_match<E: SolEventInterface>(
        items: &[&str],
        expected: impl IntoIterator<Item = (B256, usize)>,
    ) {
        let abi = JsonAbi::parse(items.iter().copied()).unwrap();
        let expected = expected.into_iter().collect::<BTreeSet<_>>();
        assert_eq!(expected.len(), E::COUNT);
        assert_eq!(event_keys(&abi), expected);
    }

    #[test]
    fn handwritten_dynamic_abi_matches_the_pinned_interfaces() {
        assert_matches_interfaces::<IB20Factory::IB20FactoryCalls, IB20Factory::IB20FactoryErrors>(
            FACTORY_ABI,
        );
        assert_events_match::<IB20Factory::IB20FactoryEvents>(
            FACTORY_ABI,
            [event_key::<IB20Factory::B20Created>()],
        );
        assert_matches_interfaces::<
            IActivationRegistry::IActivationRegistryCalls,
            IActivationRegistry::IActivationRegistryErrors,
        >(ACTIVATION_ABI);
        assert_events_match::<IActivationRegistry::IActivationRegistryEvents>(
            ACTIVATION_ABI,
            [
                event_key::<IActivationRegistry::FeatureActivated>(),
                event_key::<IActivationRegistry::FeatureDeactivated>(),
            ],
        );
        assert_matches_interfaces::<
            IPolicyRegistry::IPolicyRegistryCalls,
            IPolicyRegistry::IPolicyRegistryErrors,
        >(POLICY_ABI);
        assert_events_match::<IPolicyRegistry::IPolicyRegistryEvents>(
            POLICY_ABI,
            [
                event_key::<IPolicyRegistry::PolicyCreated>(),
                event_key::<IPolicyRegistry::PolicyAdminStaged>(),
                event_key::<IPolicyRegistry::PolicyAdminUpdated>(),
                event_key::<IPolicyRegistry::AllowlistUpdated>(),
                event_key::<IPolicyRegistry::BlocklistUpdated>(),
            ],
        );
        assert_matches_interfaces::<IB20::IB20Calls, IB20::IB20Errors>(COMMON_ABI);
        assert_events_match::<IB20::IB20Events>(
            COMMON_ABI,
            [
                event_key::<IB20::Transfer>(),
                event_key::<IB20::Approval>(),
                event_key::<IB20::Memo>(),
                event_key::<IB20::BurnedBlocked>(),
                event_key::<IB20::RoleGranted>(),
                event_key::<IB20::RoleRevoked>(),
                event_key::<IB20::RoleAdminChanged>(),
                event_key::<IB20::LastAdminRenounced>(),
                event_key::<IB20::Paused>(),
                event_key::<IB20::Unpaused>(),
                event_key::<IB20::PolicyUpdated>(),
                event_key::<IB20::SupplyCapUpdated>(),
                event_key::<IB20::ContractURIUpdated>(),
                event_key::<IB20::NameUpdated>(),
                event_key::<IB20::SymbolUpdated>(),
                event_key::<IB20::EIP712DomainChanged>(),
            ],
        );
        assert_matches_interfaces::<IB20Asset::IB20AssetCalls, IB20Asset::IB20AssetErrors>(
            ASSET_ABI,
        );
        assert_events_match::<IB20Asset::IB20AssetEvents>(
            ASSET_ABI,
            [
                event_key::<IB20Asset::MultiplierUpdated>(),
                event_key::<IB20Asset::ExtraMetadataUpdated>(),
                event_key::<IB20Asset::Announcement>(),
                event_key::<IB20Asset::EndAnnouncement>(),
            ],
        );
        let stablecoin = JsonAbi::parse(STABLECOIN_ABI.iter().copied()).unwrap();
        assert_eq!(
            function_selectors(&stablecoin),
            interface_selectors::<IB20Stablecoin::IB20StablecoinCalls>()
        );
        assert!(stablecoin.errors().next().is_none());
    }
}
