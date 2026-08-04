//! Anvil integration tests for the HashKey B20 network profile.

#[cfg(feature = "cli")]
use std::{
    fs::{self, OpenOptions},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

#[cfg(feature = "cli")]
use crate::utils::http_provider;
use alloy_eips::eip7910::EthConfig;
use alloy_genesis::Genesis;
use alloy_network::{AnyNetwork, TransactionBuilder};
use alloy_primitives::{Address, Bytes, U256, address};
use alloy_provider::Provider;
use alloy_rpc_types::{TransactionRequest, anvil::Forking};
#[cfg(feature = "cli")]
use alloy_sol_types::{SolCall, sol};
use anvil::{NodeConfig, spawn};
use foundry_evm_networks::NetworkConfigs;

const B20_FACTORY: Address = address!("B20F000000000000000000000000000000000000");
const B20_ACTIVATION_REGISTRY: Address = address!("8453000000000000000000000000000000000001");
const B20_POLICY_REGISTRY: Address = address!("8453000000000000000000000000000000000002");
const B20_FEATURE_SLOTS: [U256; 3] = [
    alloy_primitives::uint!(
        0x8c5327ddcca092db72284503162323c6e8d392394b1d5c71991227bbc26f7c07_U256
    ),
    alloy_primitives::uint!(
        0xca7c276524c5aeaac4d56c8a3d36eb5f9a64f60841fb65b539c99c21ca7df109_U256
    ),
    alloy_primitives::uint!(
        0x819420403a306232adb8ee78d9f35b5090371155b34376cf9b020e30029278e5_U256
    ),
];

#[cfg(feature = "cli")]
sol! {
    interface IB20Factory {
        function isB20(address token) external view returns (bool);
    }
}

#[cfg(feature = "cli")]
struct ChildGuard(Child);

#[cfg(feature = "cli")]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(feature = "cli")]
fn anvil_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_anvil") {
        return PathBuf::from(path);
    }

    std::env::current_exe()
        .expect("test executable path")
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target/debug directory")
        .join("anvil")
}

async fn assert_hashkey_baseline(provider: &impl Provider<AnyNetwork>) {
    for address in [B20_FACTORY, B20_ACTIVATION_REGISTRY, B20_POLICY_REGISTRY] {
        assert_eq!(provider.get_code_at(address).await.unwrap(), Bytes::from_static(&[0xef]));
    }
    for slot in B20_FEATURE_SLOTS {
        assert_eq!(
            provider.get_storage_at(B20_ACTIVATION_REGISTRY, slot).await.unwrap(),
            U256::from(1)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hashkey_standalone_reset_and_inventory() {
    let (api, handle) =
        spawn(NodeConfig::test().with_networks(NetworkConfigs::with_hashkey())).await;
    let provider = handle.http_provider();
    assert_hashkey_baseline(&provider).await;
    api.anvil_set_storage_at(B20_ACTIVATION_REGISTRY, B20_FEATURE_SLOTS[2], U256::ZERO.into())
        .await
        .unwrap();
    let snapshot = api.evm_snapshot().await.unwrap();
    api.anvil_set_storage_at(B20_ACTIVATION_REGISTRY, B20_FEATURE_SLOTS[2], U256::from(7).into())
        .await
        .unwrap();
    assert!(api.evm_revert(snapshot).await.unwrap());
    assert_eq!(
        provider.get_storage_at(B20_ACTIVATION_REGISTRY, B20_FEATURE_SLOTS[2]).await.unwrap(),
        U256::ZERO
    );

    let config: EthConfig = provider.client().request("eth_config", ()).await.unwrap();
    for (name, address) in [
        ("B20Factory", B20_FACTORY),
        ("B20ActivationRegistry", B20_ACTIVATION_REGISTRY),
        ("B20PolicyRegistry", B20_POLICY_REGISTRY),
    ] {
        assert_eq!(config.current.precompiles.get(name), Some(&address));
    }
    assert_eq!(
        config.current.precompiles.values().filter(|address| address.as_slice()[0] == 0xb2).count(),
        1,
        "static inventory must not enumerate dynamic B20 token addresses",
    );

    api.anvil_set_code(B20_FACTORY, Bytes::from_static(&[0xde, 0xad])).await.unwrap();
    api.anvil_set_storage_at(B20_ACTIVATION_REGISTRY, B20_FEATURE_SLOTS[2], U256::ZERO.into())
        .await
        .unwrap();
    api.anvil_reset(None).await.unwrap();

    assert_hashkey_baseline(&provider).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hashkey_custom_genesis_preserves_unowned_state() {
    let genesis: Genesis = serde_json::from_str(
        r#"{
          "config": {
            "chainId": 31337,
            "homesteadBlock": 0,
            "eip150Block": 0,
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "ethash": {}
          },
          "nonce": "0x0",
          "timestamp": "0x0",
          "extraData": "0x",
          "gasLimit": "0x80000000",
          "difficulty": "0x1",
          "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "coinbase": "0x0000000000000000000000000000000000000000",
          "alloc": {
            "b20f000000000000000000000000000000000000": {
              "balance": "0x2a",
              "nonce": "0x9",
              "code": "0xdead",
              "storage": {"0x0000000000000000000000000000000000000000000000000000000000000001": "0x0000000000000000000000000000000000000000000000000000000000000007"}
            },
            "4200000000000000000000000000000000000042": {
              "balance": "0x2b",
              "storage": {"0x0000000000000000000000000000000000000000000000000000000000000002": "0x0000000000000000000000000000000000000000000000000000000000000008"}
            }
          },
          "number": 0,
          "gasUsed": "0x0",
          "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000"
        }"#,
    )
    .unwrap();
    let (_api, handle) = spawn(
        NodeConfig::test()
            .with_networks(NetworkConfigs::with_hashkey())
            .with_genesis(Some(genesis)),
    )
    .await;
    let provider = handle.http_provider();

    assert_eq!(provider.get_balance(B20_FACTORY).await.unwrap(), U256::from(42));
    assert_eq!(provider.get_code_at(B20_FACTORY).await.unwrap(), Bytes::from_static(&[0xef]));
    assert_eq!(provider.get_storage_at(B20_FACTORY, U256::from(1)).await.unwrap(), U256::from(7));
    assert_eq!(
        provider.get_balance(address!("4200000000000000000000000000000000000042")).await.unwrap(),
        U256::from(43)
    );
    assert_eq!(
        provider
            .get_storage_at(address!("4200000000000000000000000000000000000042"), U256::from(2))
            .await
            .unwrap(),
        U256::from(8)
    );
    assert_eq!(
        provider.get_storage_at(B20_ACTIVATION_REGISTRY, B20_FEATURE_SLOTS[0]).await.unwrap(),
        U256::from(1)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hashkey_fork_to_standalone_discards_remote_backing() {
    let (source_api, source_handle) = spawn(NodeConfig::test()).await;
    let remote_account = address!("4200000000000000000000000000000000000042");
    let unseen_remote_account = address!("4300000000000000000000000000000000000043");
    let dynamic_b20_token = address!("B200000000000000000000000000000000000000");
    let slot = U256::from(7);
    let unseen_slot = U256::from(8);
    let remote_code = Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]);
    let remote_value = U256::from(7);
    let unseen_remote_value = U256::from(11);

    source_api.anvil_set_balance(remote_account, remote_value).await.unwrap();
    source_api.anvil_set_code(remote_account, remote_code.clone()).await.unwrap();
    source_api.anvil_set_storage_at(remote_account, slot, remote_value.into()).await.unwrap();
    source_api.anvil_set_balance(unseen_remote_account, unseen_remote_value).await.unwrap();
    source_api.anvil_set_code(unseen_remote_account, remote_code.clone()).await.unwrap();
    source_api
        .anvil_set_storage_at(unseen_remote_account, unseen_slot, unseen_remote_value.into())
        .await
        .unwrap();
    source_api.anvil_set_code(dynamic_b20_token, Bytes::from_static(&[0xef])).await.unwrap();
    source_api.anvil_set_storage_at(dynamic_b20_token, slot, remote_value.into()).await.unwrap();

    let (api, handle) = spawn(
        NodeConfig::test()
            .with_networks(NetworkConfigs::with_hashkey())
            .with_eth_rpc_url(Some(source_handle.http_endpoint())),
    )
    .await;
    let provider = handle.http_provider();
    let genesis_account = api.accounts().unwrap()[0];
    let genesis_balance = provider.get_balance(genesis_account).await.unwrap();

    assert_eq!(provider.get_balance(remote_account).await.unwrap(), remote_value);
    assert_eq!(provider.get_code_at(remote_account).await.unwrap(), remote_code);
    assert_eq!(provider.get_storage_at(remote_account, slot).await.unwrap(), remote_value);
    assert_eq!(provider.get_code_at(dynamic_b20_token).await.unwrap(), Bytes::from_static(&[0xef]));
    assert_eq!(provider.get_storage_at(dynamic_b20_token, slot).await.unwrap(), remote_value);

    api.anvil_reset(None).await.unwrap();

    assert_eq!(provider.get_balance(remote_account).await.unwrap(), U256::ZERO);
    assert!(provider.get_code_at(remote_account).await.unwrap().is_empty());
    assert_eq!(provider.get_storage_at(remote_account, slot).await.unwrap(), U256::ZERO);
    assert_eq!(provider.get_balance(unseen_remote_account).await.unwrap(), U256::ZERO);
    assert!(provider.get_code_at(unseen_remote_account).await.unwrap().is_empty());
    assert_eq!(
        provider.get_storage_at(unseen_remote_account, unseen_slot).await.unwrap(),
        U256::ZERO
    );
    assert!(provider.get_code_at(dynamic_b20_token).await.unwrap().is_empty());
    assert_eq!(provider.get_storage_at(dynamic_b20_token, slot).await.unwrap(), U256::ZERO);
    assert_eq!(provider.get_balance(genesis_account).await.unwrap(), genesis_balance);
    assert_hashkey_baseline(&provider).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hashkey_fork_preserves_remote_state_across_reset() {
    let (source_api, source_handle) = spawn(NodeConfig::test()).await;
    let remote_code = Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]);
    let remote_feature_value = U256::from(7);
    source_api.anvil_set_code(B20_FACTORY, remote_code.clone()).await.unwrap();
    source_api
        .anvil_set_storage_at(
            B20_ACTIVATION_REGISTRY,
            B20_FEATURE_SLOTS[2],
            remote_feature_value.into(),
        )
        .await
        .unwrap();

    let (api, handle) = spawn(
        NodeConfig::test()
            .with_networks(NetworkConfigs::with_hashkey())
            .with_eth_rpc_url(Some(source_handle.http_endpoint())),
    )
    .await;
    let provider = handle.http_provider();

    assert_eq!(provider.get_code_at(B20_FACTORY).await.unwrap(), remote_code);
    assert_eq!(
        provider.get_storage_at(B20_ACTIVATION_REGISTRY, B20_FEATURE_SLOTS[2]).await.unwrap(),
        remote_feature_value,
    );
    assert!(provider.get_code_at(B20_POLICY_REGISTRY).await.unwrap().is_empty());

    api.anvil_reset(Some(Forking::default())).await.unwrap();

    assert_eq!(provider.get_code_at(B20_FACTORY).await.unwrap(), remote_code);
    assert_eq!(
        provider.get_storage_at(B20_ACTIVATION_REGISTRY, B20_FEATURE_SLOTS[2]).await.unwrap(),
        remote_feature_value,
    );
}

#[cfg(feature = "cli")]
#[tokio::test(flavor = "multi_thread")]
async fn hashkey_cli_starts_with_b20_baseline() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let port_arg = port.to_string();

    let mut child = ChildGuard(
        Command::new(anvil_binary())
            .args(["--network", "hashkey", "--host", "127.0.0.1", "--port", &port_arg, "-q"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn anvil --network hashkey"),
    );

    let provider = http_provider(&format!("http://127.0.0.1:{port}"));
    let mut ready = false;
    for _ in 0..100 {
        if provider.get_chain_id().await.is_ok() {
            ready = true;
            break;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("anvil exited before serving RPC: {status}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "anvil --network hashkey should start serving RPC");

    assert_hashkey_baseline(&provider).await;
}

#[cfg(feature = "cli")]
#[tokio::test(flavor = "multi_thread")]
async fn hashkey_cli_prints_profile_aware_b20_traces() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let port_arg = port.to_string();

    // Redirect the child stdout to a file: file writes survive a kill, so the trace can be
    // synchronized on deterministically instead of racing a fixed delay against the pipe. The
    // profile-aware trace is printed on stdout; interleaving stderr into the same file could
    // split trace lines, so stderr is discarded.
    let trace_log = std::env::temp_dir().join(format!("anvil-hashkey-trace-{port}.log"));
    let stdout_file = OpenOptions::new().create(true).append(true).open(&trace_log).unwrap();

    let mut child = ChildGuard(
        Command::new(anvil_binary())
            .args([
                "--network",
                "hashkey",
                "--host",
                "127.0.0.1",
                "--port",
                &port_arg,
                "--print-traces",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn anvil --network hashkey --print-traces"),
    );

    let provider = http_provider(&format!("http://127.0.0.1:{port}"));
    let mut ready = false;
    for _ in 0..100 {
        if provider.get_chain_id().await.is_ok() {
            ready = true;
            break;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("anvil exited before serving RPC: {status}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "anvil --network hashkey should start serving RPC");

    let factory = B20_FACTORY;
    let tx = TransactionRequest::default()
        .to(factory)
        .with_input(IB20Factory::isB20Call { token: Address::repeat_byte(0x11) }.abi_encode());
    provider.call(tx.into()).await.unwrap();

    // Synchronize on the trace output instead of sleeping: the trace may not have reached the
    // pipe when the child is killed, which made a fixed delay flaky. Poll the log until the
    // complete trace appears, then shut the node down. The trace printer emits ANSI styles even
    // when redirected, so styles are stripped before matching.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut output = strip_ansi(&fs::read_to_string(&trace_log).unwrap_or_default());
    while !output.contains("← [Return] false") {
        assert!(
            std::time::Instant::now() < deadline,
            "anvil did not print the B20 trace; log={output:?}",
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        output = strip_ansi(&fs::read_to_string(&trace_log).unwrap_or_default());
    }

    child.0.kill().unwrap();
    child.0.wait().unwrap();
    // Re-read after shutdown to capture any remaining output.
    output = strip_ansi(&fs::read_to_string(&trace_log).unwrap_or_default());
    let trace = output[output.find("Traces=\n").expect("anvil printed the trace")..].trim_end();
    let _ = fs::remove_file(&trace_log);
    assert_eq!(
        trace,
        r#"Traces=
  [12] B20Factory::isB20(0x1111111111111111111111111111111111111111)
    └─ ← [Return] false"#
    );
}

/// Removes ANSI escape sequences (SGR parameter sequences) from the given text.
#[cfg(feature = "cli")]
fn strip_ansi(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            stripped.push(c);
        }
    }
    stripped
}
