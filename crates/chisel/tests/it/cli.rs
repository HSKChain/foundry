//! CLI-level tests for command network profile resolution.
//!
//! These tests spawn the `chisel` binary directly and assert on its stdout, stderr, and exit
//! status. They cover configured-chain mapping and resolution failures without entering the REPL.

use foundry_compilers::PathStyle;
use foundry_test_utils::TestProject;
use std::process::{Command, Output};

/// Spawns a `chisel` command in an isolated project directory.
fn chisel_cli(name: &str) -> (TestProject, Command) {
    let project = TestProject::new(name, PathStyle::Dapptools);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chisel"));
    cmd.current_dir(project.root());
    cmd.env("NO_COLOR", "1");
    cmd.env("ETHERSCAN_API_KEY", foundry_test_utils::rpc::next_etherscan_api_key());
    (project, cmd)
}

/// Runs the command and returns its output.
#[track_caller]
fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("failed to run chisel")
}

/// The PATH_USD TIP-20 token address. Under the Tempo profile it is initialized during EVM
/// genesis; under any other profile the address is empty.
const PATH_USD_ADDRESS: &str = "0x20C0000000000000000000000000000000000000";

// A configured Tempo chain identity selects the Tempo profile, which initializes Tempo
// precompiles and TIP-20 tokens during EVM construction.
#[test]
fn configured_tempo_chain_hint_selects_tempo() {
    let (_project, mut cmd) = chisel_cli("chisel_cli_configured_tempo_chain");
    let output = run(cmd.args([
        "--chain",
        "42431",
        "eval",
        &format!("address({PATH_USD_ADDRESS}).code.length == 0"),
    ]));

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Type: bool\n└ Value: false\n");
}

// Without a chain selection the default Ethereum profile leaves the PATH_USD address empty.
#[test]
fn default_profile_is_ethereum() {
    let (_project, mut cmd) = chisel_cli("chisel_cli_default_ethereum");
    let output = run(cmd.args(["eval", &format!("address({PATH_USD_ADDRESS}).code.length == 0")]));

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Type: bool\n└ Value: true\n");
}

// A required fork identity transport failure stops Chisel before session creation with exact
// stderr and empty stdout.
#[test]
fn fork_identity_unavailable_fails_before_session() {
    let (_project, mut cmd) = chisel_cli("chisel_cli_fork_identity_unavailable");
    let output = run(cmd.args(["--fork-url", "http://127.0.0.1:1", "eval", "uint256(1)"]));

    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Error: failed to resolve network profile from fork identity: fork identity transport \
         unavailable: eth_chainId request failed\n"
    );
}
