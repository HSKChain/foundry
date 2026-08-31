//! Anvil CLI tests for canonical command profile resolution.
//!
//! These tests exercise the private Anvil role adapter over the shared resolver:
//! standalone configured chain or genesis chain identity participates as a network
//! profile hint, while a fork `--chain-id` remains an execution override and cannot
//! replace the remote fork network identity.

use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use alloy_rpc_types::TransactionRequest;
use alloy_serde::WithOtherFields;
use anvil::{NodeConfig, spawn};
use foundry_test_utils::init_tracing;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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

fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

struct CliNode {
    // Kept alive for the node's lifetime; dropped to shut the process down.
    #[allow(dead_code)]
    child: ChildGuard,
    endpoint: String,
}

async fn spawn_cli_node(args: &[&str]) -> CliNode {
    let port = free_port();
    let port_arg = port.to_string();
    let mut child = ChildGuard(
        Command::new(anvil_binary())
            .args(args)
            .args(["--host", "127.0.0.1", "--port", &port_arg, "-q"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn anvil CLI"),
    );

    let provider = crate::utils::http_provider(&format!("http://127.0.0.1:{port}"));
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
    assert!(ready, "anvil CLI should start serving RPC");

    CliNode { child, endpoint: format!("http://127.0.0.1:{port}") }
}

/// Sends a native value transfer and returns the RPC error message.
async fn send_native_transfer(endpoint: &str) -> String {
    let provider = crate::utils::http_provider(endpoint);
    let from: Address = provider
        .get_accounts()
        .await
        .expect("dev accounts")
        .first()
        .copied()
        .expect("at least one dev account");
    let to = Address::repeat_byte(0x42);

    let tx = TransactionRequest::default().from(from).to(to).value(U256::from(1u64));
    let result = provider.send_transaction(WithOtherFields::new(tx)).await;
    match result {
        Ok(_) => String::new(),
        Err(err) => err.to_string(),
    }
}

// ============================================================================
// Standalone hint.
// ============================================================================

// A standalone configured Tempo chain identity participates as a network profile hint and
// selects Tempo semantics without any fork endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn standalone_chain_id_hint_selects_tempo_profile() {
    init_tracing();
    let node = spawn_cli_node(&["--chain-id", "42431"]).await;
    let provider = crate::utils::http_provider(&node.endpoint);

    assert_eq!(provider.get_chain_id().await.unwrap(), 42431);

    let err = send_native_transfer(&node.endpoint).await;
    assert!(
        err.contains("native value transfer not allowed"),
        "expected Tempo rejection, got: {err}"
    );
}

// A standalone configured Ethereum chain identity selects the Ethereum profile.
#[tokio::test(flavor = "multi_thread")]
async fn standalone_chain_id_hint_selects_ethereum_profile() {
    init_tracing();
    let node = spawn_cli_node(&["--chain-id", "1"]).await;
    let provider = crate::utils::http_provider(&node.endpoint);

    assert_eq!(provider.get_chain_id().await.unwrap(), 1);

    let err = send_native_transfer(&node.endpoint).await;
    assert!(err.is_empty(), "plain Ethereum must accept native transfers, got: {err}");
}

// ============================================================================
// Fork override vs fork identity.
// ============================================================================

// A fork `--chain-id` remains an execution override: `eth_chainId` reports the override while
// the remote fork identity still selects the Tempo profile.
#[tokio::test(flavor = "multi_thread")]
async fn fork_chain_id_override_keeps_remote_fork_identity() {
    init_tracing();
    let (_source_api, source_handle) =
        spawn(NodeConfig::test_tempo().with_chain_id(Some(42431u64))).await;

    let node =
        spawn_cli_node(&["--fork-url", &source_handle.http_endpoint(), "--chain-id", "31337"])
            .await;
    let provider = crate::utils::http_provider(&node.endpoint);

    // Execution chain override is visible on the wire.
    assert_eq!(provider.get_chain_id().await.unwrap(), 31337);

    // Tempo semantics come from the remote fork identity, not the override.
    let err = send_native_transfer(&node.endpoint).await;
    assert!(
        err.contains("native value transfer not allowed"),
        "expected Tempo rejection from fork identity, got: {err}"
    );
}

// Without any explicit selection, a Tempo fork endpoint selects Tempo through shared fork
// identity resolution and inherits the remote execution chain.
#[tokio::test(flavor = "multi_thread")]
async fn fork_tempo_endpoint_selects_tempo_without_chain_id() {
    init_tracing();
    let (_source_api, source_handle) =
        spawn(NodeConfig::test_tempo().with_chain_id(Some(42431u64))).await;

    let node = spawn_cli_node(&["--fork-url", &source_handle.http_endpoint()]).await;
    let provider = crate::utils::http_provider(&node.endpoint);

    assert_eq!(provider.get_chain_id().await.unwrap(), 42431);

    let err = send_native_transfer(&node.endpoint).await;
    assert!(
        err.contains("native value transfer not allowed"),
        "expected Tempo rejection from fork identity, got: {err}"
    );
}

// ============================================================================
// CLI resolution failure.
// ============================================================================

// A required fork identity transport failure stops the node before setup with exact stderr
// and empty stdout.
#[tokio::test(flavor = "multi_thread")]
async fn fork_identity_unavailable_fails_with_exact_stderr() {
    init_tracing();
    let port = free_port();
    let port_arg = port.to_string();

    let output = Command::new(anvil_binary())
        .args(["--fork-url", "http://127.0.0.1:1", "--host", "127.0.0.1", "--port", &port_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run anvil CLI");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "stdout must stay empty on resolution failure");

    let mut stderr = String::new();
    stderr.push_str(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        stderr,
        "Error: failed to resolve network profile from fork identity: fork identity transport \
         unavailable: eth_chainId request failed\n",
        "unexpected stderr",
    );
}
