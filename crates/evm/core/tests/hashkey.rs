#![cfg(feature = "hashkey")]

use alloy_evm::{Evm, EvmEnv};
use alloy_op_evm::{OpEvmFactory, OpTx};
use alloy_primitives::{Address, B256, Bytes, TxKind, U256, address, b256, keccak256};
use alloy_sol_types::{SolCall, SolError, SolValue};
use foundry_evm_core::{
    FoundryBlock, FoundryContextExt, InspectorExt,
    backend::{Backend, construction as backend_construction},
    evm::{FoundryEvmFactory, OpEvmNetwork},
};
use foundry_evm_networks::{HSK_H20_LOCAL_ADMIN, NetworkConfigs, ResolvedNetworkProfile};
use hsk_h20_precompiles::{
    ActivationFeature, H20Variant, IActivationRegistry, IH20, IH20Factory, IH20Stablecoin,
    IPolicyRegistry,
};
use op_revm::OpTransaction;
use revm::{
    DatabaseCommit, DatabaseRef, Inspector,
    context::{BlockEnv, CfgEnv, TxEnv, result::ExecutionResult},
    state::{AccountInfo, EvmState},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const CHAIN_ID: u64 = 177;
const CALLER: Address = address!("1111111111111111111111111111111111111111");
const H20_FACTORY: Address = address!("0177FF0000000000000000000000000000000000");
const H20_ACTIVATION_REGISTRY: Address = address!("0177FF0000000000000000000000000000000001");
const H20_POLICY_REGISTRY: Address = address!("0177FF0000000000000000000000000000000002");
const POLICY_FEATURE_SLOT: U256 = alloy_primitives::uint!(
    0x8b392998db41c7a56188d244d46e8d66c0e0050b53b0bc8c714ab53aedfe76c7_U256
);
const ASSET: Address = address!("017700000000000000000066c4330a000f141455");
const STABLECOIN: Address = address!("017700000000000000000166ab25bbf43b4010ce");
const MARKER_CODE_HASH: B256 =
    b256!("309b8896ee4c1ff7ec1966155373dee42663b6b40c3fedc70ba501684848d2a3");
const EMPTY_OBSERVABLE_HASH: B256 =
    b256!("011b4d03dd8c01f1049143cf9c4c817e4b167f1d1b83e5c6f0f10d89ba1e7bce");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum H20Scenario {
    CreateAsset,
    ReadAsset,
    CreateStablecoin,
    ReadStablecoin,
    CreatePolicy,
    ReadActivation,
    DuplicateAsset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutcomeStatus {
    Success,
    Revert,
    Halt,
}

#[derive(Debug, PartialEq, Eq)]
struct ConformanceRun {
    results: Vec<ExecutionResult<op_revm::OpHaltReason>>,
    state_diffs: Vec<EvmState>,
}

#[derive(Debug, PartialEq, Eq)]
struct ScenarioObservable {
    scenario: H20Scenario,
    status: OutcomeStatus,
    output: Bytes,
    gas_used: u64,
    logs_hash: B256,
    storage_hash: B256,
}

#[derive(Clone, Copy)]
struct ProfileInspector {
    profile: ResolvedNetworkProfile,
}

impl InspectorExt for ProfileInspector {
    fn get_network_profile(&self) -> ResolvedNetworkProfile {
        self.profile
    }
}

impl<CTX> Inspector<CTX> for ProfileInspector {}

#[derive(Clone)]
struct RecordingProfileInspector {
    profile: ResolvedNetworkProfile,
    calls: Arc<AtomicUsize>,
}

impl InspectorExt for RecordingProfileInspector {
    fn get_network_profile(&self) -> ResolvedNetworkProfile {
        self.profile
    }
}

impl<CTX> Inspector<CTX> for RecordingProfileInspector {
    fn call(
        &mut self,
        _context: &mut CTX,
        _inputs: &mut revm::interpreter::CallInputs,
    ) -> Option<revm::interpreter::CallOutcome> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        None
    }
}

fn backend(profile: ResolvedNetworkProfile) -> Backend<OpEvmNetwork> {
    let mut backend = backend_construction::spawn(None, profile).unwrap();
    backend.insert_account_info(CALLER, AccountInfo { balance: U256::MAX, ..Default::default() });
    backend
}

fn evm_env(timestamp: u64) -> EvmEnv<op_revm::OpSpecId, BlockEnv> {
    let mut cfg = CfgEnv::default();
    cfg.chain_id = CHAIN_ID;
    let block = BlockEnv { timestamp: U256::from(timestamp), ..Default::default() };
    EvmEnv::new(cfg, block)
}

fn tx(nonce: u64, target: Address, data: Vec<u8>) -> OpTx {
    OpTx(
        OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .caller(CALLER)
                    .gas_limit(5_000_000)
                    .gas_price(0)
                    .kind(TxKind::Call(target))
                    .data(data.into())
                    .nonce(nonce)
                    .chain_id(Some(CHAIN_ID)),
            )
            .build_fill(),
    )
}

fn scenario_transactions() -> Vec<(H20Scenario, OpTx)> {
    let asset_params = IH20Factory::H20AssetCreateParams {
        version: 1,
        name: "Conformance Asset".to_string(),
        symbol: "CFA".to_string(),
        initialAdmin: CALLER,
        decimals: 18,
    }
    .abi_encode();
    let stablecoin_params = IH20Factory::H20StablecoinCreateParams {
        version: 1,
        name: "Conformance Dollar".to_string(),
        symbol: "CFD".to_string(),
        initialAdmin: CALLER,
        currency: "USD".to_string(),
    }
    .abi_encode();

    vec![
        (
            H20Scenario::CreateAsset,
            tx(
                0,
                H20_FACTORY,
                IH20Factory::createH20Call {
                    variant: IH20Factory::H20Variant::ASSET,
                    salt: B256::from(U256::from(0xa1)),
                    params: asset_params.clone().into(),
                    initCalls: Vec::new(),
                }
                .abi_encode(),
            ),
        ),
        (H20Scenario::ReadAsset, tx(1, ASSET, IH20::nameCall {}.abi_encode())),
        (
            H20Scenario::CreateStablecoin,
            tx(
                2,
                H20_FACTORY,
                IH20Factory::createH20Call {
                    variant: IH20Factory::H20Variant::STABLECOIN,
                    salt: B256::from(U256::from(0xb2)),
                    params: stablecoin_params.into(),
                    initCalls: Vec::new(),
                }
                .abi_encode(),
            ),
        ),
        (
            H20Scenario::ReadStablecoin,
            tx(3, STABLECOIN, IH20Stablecoin::currencyCall {}.abi_encode()),
        ),
        (
            H20Scenario::CreatePolicy,
            tx(
                4,
                H20_POLICY_REGISTRY,
                IPolicyRegistry::createPolicyCall {
                    admin: CALLER,
                    policyType: IPolicyRegistry::PolicyType::BLOCKLIST,
                }
                .abi_encode(),
            ),
        ),
        (
            H20Scenario::ReadActivation,
            tx(
                5,
                H20_ACTIVATION_REGISTRY,
                IActivationRegistry::isActivatedCall { feature: ActivationFeature::H20Asset.id() }
                    .abi_encode(),
            ),
        ),
        (
            H20Scenario::DuplicateAsset,
            tx(
                6,
                H20_FACTORY,
                IH20Factory::createH20Call {
                    variant: IH20Factory::H20Variant::ASSET,
                    salt: B256::from(U256::from(0xa1)),
                    params: asset_params.into(),
                    initCalls: Vec::new(),
                }
                .abi_encode(),
            ),
        ),
    ]
}

fn execute<I>(inspector: I) -> ConformanceRun
where
    I: for<'db> foundry_evm_core::FoundryInspectorExt<
            alloy_op_evm::OpEvmContext<
                &'db mut dyn foundry_evm_core::backend::DatabaseExt<OpEvmFactory>,
            >,
        > + Clone,
{
    let profile = NetworkConfigs::with_hashkey().resolve();
    let mut backend = backend(profile);
    let mut evm = OpEvmFactory::default().create_foundry_evm_with_inspector(
        &mut backend,
        evm_env(0),
        inspector,
    );
    let mut results = Vec::new();
    let mut state_diffs = Vec::new();
    for (_, transaction) in scenario_transactions() {
        let result = evm.transact(transaction).unwrap();
        evm.db_mut().commit(result.state.clone());
        results.push(result.result);
        state_diffs.push(result.state);
    }
    ConformanceRun { results, state_diffs }
}

const fn outcome_status(result: &ExecutionResult<op_revm::OpHaltReason>) -> OutcomeStatus {
    match result {
        ExecutionResult::Success { .. } => OutcomeStatus::Success,
        ExecutionResult::Revert { .. } => OutcomeStatus::Revert,
        ExecutionResult::Halt { .. } => OutcomeStatus::Halt,
    }
}

fn hash_logs(result: &ExecutionResult<op_revm::OpHaltReason>) -> B256 {
    // Length-prefix every ordered component so the hash is an unambiguous exact log snapshot.
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(result.logs().len() as u64).to_be_bytes());
    for log in result.logs() {
        encoded.extend_from_slice(log.address.as_slice());
        encoded.extend_from_slice(&(log.topics().len() as u64).to_be_bytes());
        for topic in log.topics() {
            encoded.extend_from_slice(topic.as_slice());
        }
        encoded.extend_from_slice(&(log.data.data.len() as u64).to_be_bytes());
        encoded.extend_from_slice(&log.data.data);
    }
    keccak256(encoded)
}

fn is_h20_address(address: Address) -> bool {
    matches!(address, H20_FACTORY | H20_ACTIVATION_REGISTRY | H20_POLICY_REGISTRY)
        || H20Variant::from_address(address).is_some()
}

fn hash_storage_diff(state: &EvmState) -> B256 {
    // Normalize H20-only changes by address and slot, excluding journal-local access metadata.
    let mut accounts: Vec<_> = state
        .iter()
        .filter(|(address, account)| {
            is_h20_address(**address) && account.storage.values().any(|slot| slot.is_changed())
        })
        .collect();
    accounts.sort_by_key(|(address, _)| **address);

    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(accounts.len() as u64).to_be_bytes());
    for (address, account) in accounts {
        encoded.extend_from_slice(address.as_slice());
        encoded.extend_from_slice(&account.info.balance.to_be_bytes::<32>());
        encoded.extend_from_slice(&account.info.nonce.to_be_bytes());
        encoded.extend_from_slice(account.info.code_hash.as_slice());

        let mut slots: Vec<_> =
            account.storage.iter().filter(|(_, slot)| slot.is_changed()).collect();
        slots.sort_by_key(|(slot, _)| **slot);
        encoded.extend_from_slice(&(slots.len() as u64).to_be_bytes());
        for (slot, value) in slots {
            encoded.extend_from_slice(&slot.to_be_bytes::<32>());
            encoded.extend_from_slice(&value.original_value.to_be_bytes::<32>());
            encoded.extend_from_slice(&value.present_value.to_be_bytes::<32>());
        }
    }
    keccak256(encoded)
}

fn observables(run: &ConformanceRun) -> Vec<ScenarioObservable> {
    scenario_transactions()
        .into_iter()
        .zip(&run.results)
        .zip(&run.state_diffs)
        .map(|(((scenario, _), result), state)| ScenarioObservable {
            scenario,
            status: outcome_status(result),
            output: result.output().cloned().unwrap_or_default(),
            gas_used: result.tx_gas_used(),
            logs_hash: hash_logs(result),
            storage_hash: hash_storage_diff(state),
        })
        .collect()
}

fn expected_observables() -> Vec<ScenarioObservable> {
    vec![
        ScenarioObservable {
            scenario: H20Scenario::CreateAsset,
            status: OutcomeStatus::Success,
            output: IH20Factory::createH20Call::abi_encode_returns(&ASSET).into(),
            gas_used: 201_307,
            logs_hash: b256!("d47e55e3dca582c8f7cbd5632f8ded131d983f3b1202a550f09383436ca9f1c3"),
            storage_hash: b256!("a2d2a9ead506ab0bb9f0a22666674fcc6890d459a4bcfea425554d663c690299"),
        },
        ScenarioObservable {
            scenario: H20Scenario::ReadAsset,
            status: OutcomeStatus::Success,
            output: IH20::nameCall::abi_encode_returns(&"Conformance Asset".to_string()).into(),
            gas_used: 23_270,
            logs_hash: EMPTY_OBSERVABLE_HASH,
            storage_hash: EMPTY_OBSERVABLE_HASH,
        },
        ScenarioObservable {
            scenario: H20Scenario::CreateStablecoin,
            status: OutcomeStatus::Success,
            output: IH20Factory::createH20Call::abi_encode_returns(&STABLECOIN).into(),
            gas_used: 200_739,
            logs_hash: b256!("5abc3be92356760ae7a2095e381cb4d8abf794e4f73e44b9b150320546befbf8"),
            storage_hash: b256!("3da039832deed89423ae959c91f734ed0d5bf301b17ec4821a4afeef3e532edf"),
        },
        ScenarioObservable {
            scenario: H20Scenario::ReadStablecoin,
            status: OutcomeStatus::Success,
            output: IH20Stablecoin::currencyCall::abi_encode_returns(&"USD".to_string()).into(),
            gas_used: 23_270,
            logs_hash: EMPTY_OBSERVABLE_HASH,
            storage_hash: EMPTY_OBSERVABLE_HASH,
        },
        ScenarioObservable {
            scenario: H20Scenario::CreatePolicy,
            status: OutcomeStatus::Success,
            output: IPolicyRegistry::createPolicyCall::abi_encode_returns(&2).into(),
            gas_used: 116_009,
            logs_hash: b256!("1ac76809bdaf74c91b5488dd387690a62bddab5d2d8043d61fb22de7e74b6fb5"),
            storage_hash: b256!("30b61663b013681a8b07bb95a074050cbca49303e75818c8a12b79a4f9e290c9"),
        },
        ScenarioObservable {
            scenario: H20Scenario::ReadActivation,
            status: OutcomeStatus::Success,
            output: IActivationRegistry::isActivatedCall::abi_encode_returns(&true).into(),
            gas_used: 23_688,
            logs_hash: EMPTY_OBSERVABLE_HASH,
            storage_hash: EMPTY_OBSERVABLE_HASH,
        },
        ScenarioObservable {
            scenario: H20Scenario::DuplicateAsset,
            status: OutcomeStatus::Revert,
            output: IH20Factory::TokenAlreadyExists { token: ASSET }.abi_encode().into(),
            gas_used: 28_592,
            logs_hash: EMPTY_OBSERVABLE_HASH,
            storage_hash: EMPTY_OBSERVABLE_HASH,
        },
    ]
}

#[test]
fn normal_and_inspected_h20_scenarios_are_observably_identical() {
    let profile = NetworkConfigs::with_hashkey().resolve();
    let normal = execute(ProfileInspector { profile });
    let inspected_calls = Arc::new(AtomicUsize::new(0));
    let inspected = execute(RecordingProfileInspector { profile, calls: inspected_calls.clone() });

    assert!(inspected_calls.load(Ordering::Relaxed) >= scenario_transactions().len());
    assert_eq!(normal, inspected);
    assert_eq!(observables(&normal), expected_observables());

    let asset_output = normal.results[0].output().unwrap();
    assert_eq!(IH20Factory::createH20Call::abi_decode_returns(asset_output).unwrap(), ASSET);
    assert!(normal.results[0].is_success());
    assert!(!normal.results[0].logs().is_empty());
    let asset_diff = normal.state_diffs[0].get(&ASSET).expect("asset creation state diff");
    assert_eq!(asset_diff.info.code_hash, MARKER_CODE_HASH);
    assert_eq!(asset_diff.info.nonce, 0);
    assert!(!asset_diff.storage.is_empty());

    assert_eq!(
        IH20::nameCall::abi_decode_returns(normal.results[1].output().unwrap()).unwrap(),
        "Conformance Asset"
    );
    assert_eq!(
        IH20Factory::createH20Call::abi_decode_returns(normal.results[2].output().unwrap())
            .unwrap(),
        STABLECOIN
    );
    assert_eq!(
        IH20Stablecoin::currencyCall::abi_decode_returns(normal.results[3].output().unwrap())
            .unwrap(),
        "USD"
    );
    assert_eq!(
        IPolicyRegistry::createPolicyCall::abi_decode_returns(normal.results[4].output().unwrap())
            .unwrap(),
        2
    );
    assert!(
        IActivationRegistry::isActivatedCall::abi_decode_returns(
            normal.results[5].output().unwrap()
        )
        .unwrap()
    );
    assert_eq!(keccak256([0xef]), MARKER_CODE_HASH);
    assert_eq!(profile.h20_config().activation_admin(), Some(HSK_H20_LOCAL_ADMIN));
}

#[test]
fn activation_snapshot_survives_warp_and_refreshes_on_later_evm_creation() {
    let config =
        hsk_h20_config::H20Config::new(Some(100), Some(Address::repeat_byte(0x11))).unwrap();
    let profile = NetworkConfigs::with_hashkey().resolve().with_h20_config_for_test(config);
    let mut backend = backend(profile);
    let activation_registry = H20_ACTIVATION_REGISTRY;
    let feature_slot = POLICY_FEATURE_SLOT;
    backend.insert_account_storage(activation_registry, feature_slot, U256::ZERO).unwrap();

    let mut created_at_99 = OpEvmFactory::default().create_foundry_evm_with_inspector(
        &mut backend,
        evm_env(99),
        ProfileInspector { profile },
    );
    assert!(created_at_99.precompiles().get(&H20_FACTORY).is_none());
    created_at_99.ctx_mut().block_mut().set_timestamp(U256::from(100));
    assert_eq!(created_at_99.ctx().block.timestamp, U256::from(100));
    assert!(created_at_99.precompiles().get(&H20_FACTORY).is_none());
    drop(created_at_99);

    for timestamp in [100, 101] {
        let later_evm = OpEvmFactory::default().create_foundry_evm_with_inspector(
            &mut backend,
            evm_env(timestamp),
            ProfileInspector { profile },
        );
        assert!(later_evm.precompiles().get(&H20_FACTORY).is_some(), "timestamp {timestamp}");
        drop(later_evm);
    }
    assert_eq!(backend.storage_ref(activation_registry, feature_slot).unwrap(), U256::ZERO);
}
