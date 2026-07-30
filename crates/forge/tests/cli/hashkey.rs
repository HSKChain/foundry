//! HashKey B20 integration fixtures.

use foundry_test_utils::forgetest_init;

forgetest_init!(hashkey_b20_asset_lifecycle, |prj, cmd| {
    prj.add_source("B20.sol", include_str!("../../../../testdata/default/hashkey/src/B20.sol"));
    prj.add_test("B20.t.sol", include_str!("../../../../testdata/default/hashkey/test/B20.t.sol"));
    prj.create_file(
        "foundry.toml",
        r#"
[default]
src = "src"
out = "out"
libs = ["lib"]
network = "hashkey"
"#,
    );

    cmd.args(["test", "--match-contract", "B20AssetTest"]).assert_success();
});
