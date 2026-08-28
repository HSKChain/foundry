//! Cast end-to-end coverage for the HashKey H20 Anvil profile.

use alloy_primitives::{Address, B256, Bytes, U256, address, hex, keccak256};
use alloy_sol_types::{SolCall, SolValue, sol};
use anvil::NodeConfig;
use foundry_evm_networks::{NetworkConfigs, NetworkTraceIdentity};
use foundry_test_utils::{str, util::OutputExt};

const DEVELOPMENT_ADMIN: Address = address!("0xCB00000000000000000000000000000000000000");
const CREATOR: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
const RECIPIENT: Address = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
const CREATOR_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const H20_ASSET_FEATURE_SLOT: U256 = alloy_primitives::uint!(
    0xccefe162b92620aa4539cf19b2b49990ae6c328fdc91d82c89e41c203b682d55_U256
);

sol! {
    struct H20AssetCreateParams {
        uint8 version;
        string name;
        string symbol;
        address initialAdmin;
        uint8 decimals;
    }

    interface IH20Factory {
        function createH20(uint8 variant, bytes32 salt, bytes params, bytes[] initCalls)
            external
            returns (address token);
        function getH20Address(uint8 variant, address sender, bytes32 salt)
            external
            view
            returns (address token);
    }
}

fn stdout(cmd: &mut foundry_test_utils::TestCommand, args: &[&str]) -> String {
    cmd.cast_fuse().args(args).assert_success().get_output().stdout_lossy()
}

fn send_async(cmd: &mut foundry_test_utils::TestCommand, args: &[&str]) -> String {
    let output = stdout(cmd, args);
    let tx_hash = output.trim().parse::<B256>().unwrap();
    tx_hash.to_string()
}

fn wait_for_receipt(cmd: &mut foundry_test_utils::TestCommand, tx_hash: &str, rpc: &str) {
    cmd.cast_fuse().args(["receipt", tx_hash, "--rpc-url", rpc]).assert_success();
}

casttest!(hashkey_h20_anvil_cast_workflow, async |prj, cmd| {
    prj.create_file(
        "foundry.toml",
        r#"
[default]
network = "hashkey"
"#,
    );
    cmd.set_current_dir(prj.root());

    let factory = NetworkTraceIdentity::H20Factory.fixed_address().unwrap().to_string();
    let activation_registry =
        NetworkTraceIdentity::H20ActivationRegistry.fixed_address().unwrap().to_string();
    let policy_registry =
        NetworkTraceIdentity::H20PolicyRegistry.fixed_address().unwrap().to_string();
    let asset_feature = keccak256("hsk.h20_asset").to_string();
    let asset_feature_slot = format!("{H20_ASSET_FEATURE_SLOT:#x}");
    let recipient = RECIPIENT.to_string();
    let admin = DEVELOPMENT_ADMIN.to_string();
    let creator = CREATOR.to_string();
    let salt = B256::from(U256::from(40)).to_string();

    let (_api, handle) =
        anvil::spawn(NodeConfig::test().with_networks(NetworkConfigs::with_hashkey())).await;
    let rpc = handle.http_endpoint();

    cmd.cast_fuse().args(["rpc", "anvil_snapshot", "--rpc-url", &rpc]).assert_json_stdout(str![[
        r#"
"0x0"
"#
    ]]);
    let snapshot = "0x0";

    let predict_data =
        IH20Factory::getH20AddressCall { variant: 0, sender: CREATOR, salt: salt.parse().unwrap() }
            .abi_encode();
    let predicted_output = stdout(
        &mut cmd,
        &["call", &factory, "--data", &hex::encode_prefixed(predict_data), "--rpc-url", &rpc],
    );
    let predicted = hex::decode(predicted_output.trim().trim_start_matches("0x")).unwrap();
    let token = Address::abi_decode(&predicted).unwrap().to_string();

    cmd.cast_fuse()
        .args(["storage", &activation_registry, &asset_feature_slot, "--rpc-url", &rpc])
        .assert_success()
        .stdout_eq(str![[r#"
0x0000000000000000000000000000000000000000000000000000000000000001

"#]]);
    cmd.cast_fuse()
        .args([
            "--json",
            "call",
            &activation_registry,
            "isActivated(bytes32)(bool)",
            &asset_feature,
            "--rpc-url",
            &rpc,
        ])
        .assert_json_stdout(str![[r#"
[true]
"#]]);

    let params = H20AssetCreateParams {
        version: 1,
        name: "Cast Asset".to_string(),
        symbol: "CAST".to_string(),
        initialAdmin: CREATOR,
        decimals: 18,
    }
    .abi_encode();
    let create_data = IH20Factory::createH20Call {
        variant: 0,
        salt: salt.parse().unwrap(),
        params: Bytes::from(params),
        initCalls: Vec::new(),
    }
    .abi_encode();
    let create_tx = send_async(
        &mut cmd,
        &[
            "send",
            &factory,
            "--data",
            &hex::encode_prefixed(create_data),
            "--private-key",
            CREATOR_PRIVATE_KEY,
            "--rpc-url",
            &rpc,
            "--async",
        ],
    );
    wait_for_receipt(&mut cmd, &create_tx, &rpc);
    assert_eq!(stdout(&mut cmd, &["code", &token, "--rpc-url", &rpc]).trim(), "0xef");

    let mint_role = stdout(&mut cmd, &["call", &token, "MINT_ROLE()(bytes32)", "--rpc-url", &rpc]);
    let grant_tx = send_async(
        &mut cmd,
        &[
            "send",
            &token,
            "grantRole(bytes32,address)",
            mint_role.trim(),
            &creator,
            "--private-key",
            CREATOR_PRIVATE_KEY,
            "--rpc-url",
            &rpc,
            "--async",
        ],
    );
    wait_for_receipt(&mut cmd, &grant_tx, &rpc);

    let mint_tx = send_async(
        &mut cmd,
        &[
            "send",
            &token,
            "mint(address,uint256)",
            &recipient,
            "42",
            "--private-key",
            CREATOR_PRIVATE_KEY,
            "--rpc-url",
            &rpc,
            "--async",
        ],
    );
    wait_for_receipt(&mut cmd, &mint_tx, &rpc);

    cmd.cast_fuse()
        .args([
            "--json",
            "call",
            &token,
            "balanceOf(address)(uint256)",
            &recipient,
            "--rpc-url",
            &rpc,
        ])
        .assert_json_stdout(str![[r#"
[42]
"#]]);

    cmd.cast_fuse()
        .args([
            "call",
            &token,
            "balanceOf(address)(uint256)",
            &recipient,
            "--trace",
            "--rpc-url",
            &rpc,
        ])
        .assert_success()
        .stdout_eq(str![[r#"
Traces:
  [..] H20Asset::balanceOf([..])
    └─ ← [Return] 42


Transaction successfully executed.
[GAS]

"#]]);

    cmd.cast_fuse()
        .args(["run", &mint_tx, "--network", "hashkey", "--rpc-url", &rpc])
        .assert_success()
        .stdout_eq(str![[r#"
Executing previous transactions from the block.
Traces:
  [..] H20Asset::mint([..], 42)
    ├─ emit Transfer(from: 0x0000000000000000000000000000000000000000, to: [..], amount: 42)
    └─ ← [Return]


Transaction successfully executed.
[GAS]

"#]]);

    cmd.cast_fuse()
        .args([
            "call",
            &token,
            "mint(address,uint256)",
            &recipient,
            "1",
            "--from",
            &recipient,
            "--trace",
            "--rpc-url",
            &rpc,
        ])
        .assert_success()
        .stdout_eq(str![[r#"
Traces:
  [4318] H20Asset::mint(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 1)
    └─ ← [Revert] AccessControlUnauthorizedAccount(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 0x154c00819833dac601ee5ddded6fda79d9d8b506b911b3dbd54cdb95fe6c3686)


[GAS]

"#]])
        .stderr_eq(str![[r#"
Error: Transaction failed.

"#]]);

    cmd.cast_fuse()
        .args(["rpc", "anvil_impersonateAccount", &admin, "--rpc-url", &rpc])
        .assert_json_stdout(str![[r#"
null
"#]]);
    let deactivate_tx = send_async(
        &mut cmd,
        &[
            "send",
            &activation_registry,
            "deactivate(bytes32)",
            &asset_feature,
            "--from",
            &admin,
            "--unlocked",
            "--rpc-url",
            &rpc,
            "--async",
        ],
    );
    wait_for_receipt(&mut cmd, &deactivate_tx, &rpc);
    cmd.cast_fuse()
        .args([
            "--json",
            "call",
            &activation_registry,
            "isActivated(bytes32)(bool)",
            &asset_feature,
            "--rpc-url",
            &rpc,
        ])
        .assert_json_stdout(str![[r#"
[false]
"#]]);
    cmd.cast_fuse()
        .args(["storage", &activation_registry, &asset_feature_slot, "--rpc-url", &rpc])
        .assert_success()
        .stdout_eq(str![[r#"
0x0000000000000000000000000000000000000000000000000000000000000000

"#]]);

    cmd.cast_fuse().args(["rpc", "anvil_revert", snapshot, "--rpc-url", &rpc]).assert_json_stdout(
        str![[r#"
true
"#]],
    );
    assert_eq!(stdout(&mut cmd, &["code", &token, "--rpc-url", &rpc]).trim(), "0x");
    cmd.cast_fuse()
        .args(["storage", &activation_registry, &asset_feature_slot, "--rpc-url", &rpc])
        .assert_success()
        .stdout_eq(str![[r#"
0x0000000000000000000000000000000000000000000000000000000000000001

"#]]);
    for singleton in [&factory, &activation_registry, &policy_registry] {
        assert_eq!(stdout(&mut cmd, &["code", singleton, "--rpc-url", &rpc]).trim(), "0xef");
    }
    cmd.cast_fuse()
        .args([
            "--json",
            "call",
            &activation_registry,
            "isActivated(bytes32)(bool)",
            &asset_feature,
            "--rpc-url",
            &rpc,
        ])
        .assert_json_stdout(str![[r#"
[true]
"#]]);
});
