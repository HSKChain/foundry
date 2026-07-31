//! Anvil integration tests for the HashKey B20 network profile.

#[cfg(feature = "cli")]
use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

#[cfg(feature = "cli")]
use crate::utils::http_provider;
use alloy_eips::eip7910::EthConfig;
use alloy_network::AnyNetwork;
use alloy_primitives::{Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types::anvil::Forking;
use anvil::{NodeConfig, spawn};
use foundry_evm_networks::{B20GenesisAlloc, NetworkConfigs};

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

fn hashkey_alloc() -> B20GenesisAlloc {
    NetworkConfigs::with_hashkey().resolve().b20_genesis_alloc().unwrap()
}

async fn assert_hashkey_baseline(provider: &impl Provider<AnyNetwork>) {
    let alloc = hashkey_alloc();
    for (address, _, _) in alloc.markers {
        assert_eq!(provider.get_code_at(address).await.unwrap(), Bytes::from_static(&[0xef]));
    }
    for (address, slot, value) in alloc.feature_seeds {
        assert_eq!(provider.get_storage_at(address, slot).await.unwrap(), value);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hashkey_standalone_reset_and_inventory() {
    let (api, handle) =
        spawn(NodeConfig::test().with_networks(NetworkConfigs::with_hashkey())).await;
    let provider = handle.http_provider();
    let alloc = hashkey_alloc();

    assert_hashkey_baseline(&provider).await;
    api.evm_snapshot().await.unwrap();
    assert_hashkey_baseline(&provider).await;

    let config: EthConfig = provider.client().request("eth_config", ()).await.unwrap();
    for (name, address) in [
        ("B20Factory", alloc.markers[0].0),
        ("B20ActivationRegistry", alloc.markers[1].0),
        ("B20PolicyRegistry", alloc.markers[2].0),
    ] {
        assert_eq!(config.current.precompiles.get(name), Some(&address));
    }
    assert_eq!(
        config.current.precompiles.values().filter(|address| address.as_slice()[0] == 0xb2).count(),
        1,
        "static inventory must not enumerate dynamic B20 token addresses",
    );

    api.anvil_set_code(alloc.markers[0].0, Bytes::from_static(&[0xde, 0xad])).await.unwrap();
    api.anvil_set_storage_at(alloc.feature_seeds[2].0, alloc.feature_seeds[2].1, U256::ZERO.into())
        .await
        .unwrap();
    api.anvil_reset(None).await.unwrap();

    assert_hashkey_baseline(&provider).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hashkey_fork_preserves_remote_state_across_reset() {
    let (source_api, source_handle) = spawn(NodeConfig::test()).await;
    let alloc = hashkey_alloc();
    let remote_code = Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]);
    let remote_feature_value = U256::from(7);
    source_api.anvil_set_code(alloc.markers[0].0, remote_code.clone()).await.unwrap();
    source_api
        .anvil_set_storage_at(
            alloc.feature_seeds[2].0,
            alloc.feature_seeds[2].1,
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

    assert_eq!(provider.get_code_at(alloc.markers[0].0).await.unwrap(), remote_code);
    assert_eq!(
        provider.get_storage_at(alloc.feature_seeds[2].0, alloc.feature_seeds[2].1).await.unwrap(),
        remote_feature_value,
    );
    assert!(provider.get_code_at(alloc.markers[2].0).await.unwrap().is_empty());

    api.anvil_reset(Some(Forking::default())).await.unwrap();

    assert_eq!(provider.get_code_at(alloc.markers[0].0).await.unwrap(), remote_code);
    assert_eq!(
        provider.get_storage_at(alloc.feature_seeds[2].0, alloc.feature_seeds[2].1).await.unwrap(),
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
