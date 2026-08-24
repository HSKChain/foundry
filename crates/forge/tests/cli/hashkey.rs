//! HashKey H20 integration fixtures.

use foundry_test_utils::{forgetest_init, str};

forgetest_init!(hashkey_h20_local_state_lifecycle, |prj, cmd| {
    prj.add_source("H20.sol", include_str!("../../../../testdata/default/hashkey/src/H20.sol"));
    prj.add_test("H20.t.sol", include_str!("../../../../testdata/default/hashkey/test/H20.t.sol"));
    prj.create_file(
        "foundry.toml",
        r#"
[profile.default]
src = "src"
out = "out"
libs = ["lib"]
network = "hashkey"
"#,
    );

    cmd.args(["test", "--match-contract", "H20LifecycleTest"]).assert_success();

    cmd.forge_fuse()
        .args(["test", "--match-test", "testAssetAndStablecoinLifecycle", "-vvvv"])
        .assert_success()
        .stdout_eq(str![[r#"
...
Traces:
...
    [..] H20Factory::createH20([..])
...
    [..] H20Asset::mint([..])
...
    [..] H20Asset::transfer([..])
...
"#]]);
});

forgetest_init!(hashkey_h20_native_state_protection, |prj, cmd| {
    prj.add_source("H20.sol", include_str!("../../../../testdata/default/hashkey/src/H20.sol"));
    prj.add_test(
        "H20Protection.t.sol",
        include_str!("../../../../testdata/default/hashkey/test/H20Protection.t.sol"),
    );
    prj.create_file(
        "foundry.toml",
        r#"
[profile.default]
src = "src"
out = "out"
libs = ["lib"]
network = "hashkey"
"#,
    );

    cmd.args(["test", "--match-contract", "H20ProtectionTest"]).assert_success();
});

forgetest_init!(hashkey_h20_script_construction, |prj, cmd| {
    prj.add_source("H20.sol", include_str!("../../../../testdata/default/hashkey/src/H20.sol"));
    let script = prj.add_script(
        "H20.s.sol",
        r#"
pragma solidity ^0.8.0;

import {H20Caller} from "../src/H20.sol";

contract H20Script {
    event AssetCreated(address asset);

    function run() external returns (address asset) {
        H20Caller caller = new H20Caller();
        asset = caller.createAsset(keccak256("script"), "Script Asset", "SCRIPT", address(0xA11CE));
        emit AssetCreated(asset);
    }
}
"#,
    );
    prj.create_file(
        "foundry.toml",
        r#"
[profile.default]
src = "src"
out = "out"
libs = ["lib"]
network = "hashkey"
"#,
    );

    cmd.arg("script").arg(script).arg("-vvvv").assert_success().stdout_eq(str![[r#"
...
Script ran successfully.
[GAS]

== Return ==
asset: address 0x0177[..]
"#]]);
});
