//! HashKey B20 integration fixtures.

use foundry_test_utils::{forgetest_init, str};

forgetest_init!(hashkey_b20_local_state_lifecycle, |prj, cmd| {
    prj.add_source("B20.sol", include_str!("../../../../testdata/default/hashkey/src/B20.sol"));
    prj.add_test("B20.t.sol", include_str!("../../../../testdata/default/hashkey/test/B20.t.sol"));
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

    cmd.args(["test", "--match-contract", "B20LifecycleTest"]).assert_success();

    cmd.forge_fuse()
        .args(["test", "--match-test", "testAssetAndStablecoinLifecycle", "-vvvv"])
        .assert_success()
        .stdout_eq(str![[r#"
...
Traces:
...
    [..] B20Factory::createB20([..])
...
    [..] B20Asset::mint([..])
...
    [..] B20Asset::transfer([..])
...
"#]]);
});

forgetest_init!(hashkey_b20_native_state_protection, |prj, cmd| {
    prj.add_source("B20.sol", include_str!("../../../../testdata/default/hashkey/src/B20.sol"));
    prj.add_test(
        "B20Protection.t.sol",
        include_str!("../../../../testdata/default/hashkey/test/B20Protection.t.sol"),
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

    cmd.args(["test", "--match-contract", "B20ProtectionTest"]).assert_success();
});

forgetest_init!(hashkey_b20_script_construction, |prj, cmd| {
    prj.add_source("B20.sol", include_str!("../../../../testdata/default/hashkey/src/B20.sol"));
    let script = prj.add_script(
        "B20.s.sol",
        r#"
pragma solidity ^0.8.0;

import {B20Caller} from "../src/B20.sol";

contract B20Script {
    event AssetCreated(address asset);

    function run() external returns (address asset) {
        B20Caller caller = new B20Caller();
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
asset: address 0xb2[..]
"#]]);
});
