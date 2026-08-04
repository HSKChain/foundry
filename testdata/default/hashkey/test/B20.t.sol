// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import {Test} from "forge-std/Test.sol";
import {
    B20Caller,
    B20Rollback,
    IActivationRegistry,
    IB20,
    IB20Factory,
    IB20Stablecoin,
    IPolicyRegistry
} from "../src/B20.sol";

contract B20LifecycleTest is Test {
    B20Caller caller;
    B20Rollback rollback;

    address constant FACTORY = 0xB20f000000000000000000000000000000000000;
    address constant ACTIVATION_REGISTRY = 0x8453000000000000000000000000000000000001;
    address constant POLICY_REGISTRY = 0x8453000000000000000000000000000000000002;
    address constant ACTIVATION_ADMIN = 0xCB00000000000000000000000000000000000000;
    address constant ALICE = address(0xA11CE);
    address constant OUTSIDER = address(0xBAD);
    bytes32 constant ASSET_FEATURE = keccak256("base.b20_asset");
    bytes32 constant SALT = bytes32(uint256(0xC0FFEE));

    function setUp() public {
        caller = new B20Caller();
        rollback = new B20Rollback();
    }

    function testAssetAndStablecoinLifecycle() public {
        address predicted = caller.predictAssetAddress(address(caller), SALT);
        address asset = caller.createAsset(SALT, "TestAsset", "TST", address(this));
        IB20 assetToken = IB20(asset);

        assertEq(asset, predicted, "deterministic address mismatch");
        assertEq(uint8(uint160(asset) >> 152), 0xb2, "asset address must have B20 prefix");
        assertEq(asset.code, hex"ef", "asset marker missing");
        assertTrue(IB20Factory(FACTORY).isB20Initialized(asset), "asset not initialized");
        assertEq(assetToken.decimals(), 18, "asset decimals mismatch");
        assetToken.grantRole(assetToken.MINT_ROLE(), address(this));
        assetToken.mint(ALICE, 1000e18);
        vm.prank(ALICE);
        assertTrue(assetToken.transfer(address(0xCAFE), 400e18), "asset transfer failed");
        assertEq(assetToken.balanceOf(ALICE), 600e18, "asset sender balance mismatch");

        address stablecoin = caller.createStablecoin(keccak256("stablecoin"), "USD Coin", "USDC", address(this), "USD");
        IB20Stablecoin stablecoinToken = IB20Stablecoin(stablecoin);

        assertEq(stablecoin.code, hex"ef", "stablecoin marker missing");
        assertTrue(IB20Factory(FACTORY).isB20Initialized(stablecoin), "stablecoin not initialized");
        assertEq(stablecoinToken.currency(), "USD", "currency mismatch");
        assertEq(stablecoinToken.decimals(), 6, "stablecoin decimals mismatch");
        stablecoinToken.grantRole(stablecoinToken.MINT_ROLE(), address(this));
        stablecoinToken.mint(ALICE, 250e6);
        vm.prank(ALICE);
        assertTrue(stablecoinToken.transfer(address(0xB0B), 50e6), "stablecoin transfer failed");
        assertEq(stablecoinToken.totalSupply(), 250e6, "stablecoin supply mismatch");
        assertEq(stablecoinToken.balanceOf(ALICE), 200e6, "stablecoin balance mismatch");
    }

    function testActivationRegistrySuccessAndTypedFailures() public {
        IActivationRegistry registry = IActivationRegistry(ACTIVATION_REGISTRY);
        assertEq(registry.admin(), ACTIVATION_ADMIN, "activation admin mismatch");
        assertTrue(registry.isActivated(ASSET_FEATURE), "asset feature not active");

        vm.startPrank(ACTIVATION_ADMIN);
        registry.deactivate(ASSET_FEATURE);
        assertTrue(!registry.isActivated(ASSET_FEATURE), "asset feature still active");

        vm.expectRevert(abi.encodeWithSelector(IActivationRegistry.FeatureNotActivated.selector, ASSET_FEATURE));
        registry.checkActivated(ASSET_FEATURE);

        registry.activate(ASSET_FEATURE);
        vm.expectRevert(abi.encodeWithSelector(IActivationRegistry.AlreadyActivated.selector, ASSET_FEATURE));
        registry.activate(ASSET_FEATURE);
        vm.stopPrank();
    }

    function testPolicyRegistrySuccessAndTypedFailure() public {
        IPolicyRegistry registry = IPolicyRegistry(POLICY_REGISTRY);
        uint64 policyId = registry.createPolicy(address(this), IPolicyRegistry.PolicyType.ALLOWLIST);
        address[] memory accounts = new address[](1);
        accounts[0] = ALICE;

        registry.updateAllowlist(policyId, true, accounts);
        assertTrue(registry.policyExists(policyId), "policy missing");
        assertEq(registry.policyAdmin(policyId), address(this), "policy admin mismatch");
        assertTrue(registry.isAuthorized(policyId, ALICE), "allowlist update missing");

        vm.startPrank(OUTSIDER);
        vm.expectRevert(IPolicyRegistry.Unauthorized.selector);
        registry.updateAllowlist(policyId, false, accounts);
        vm.stopPrank();

        assertTrue(registry.isAuthorized(policyId, ALICE), "failed update changed policy");
    }

    function testTypedMutationFailurePreservesTokenState() public {
        IB20 token = IB20(caller.createAsset(keccak256("typed"), "Typed", "TYP", address(this)));
        bytes32 mintRole = token.MINT_ROLE();

        vm.startPrank(OUTSIDER);
        vm.expectRevert(abi.encodeWithSelector(IB20.AccessControlUnauthorizedAccount.selector, OUTSIDER, mintRole));
        token.mint(ALICE, 1e18);
        vm.stopPrank();

        assertEq(token.totalSupply(), 0, "failed mint changed supply");
        assertEq(token.balanceOf(ALICE), 0, "failed mint changed balance");
    }

    function testAtomicRollbackForCreationAndMutation() public {
        bytes32 failedSalt = keccak256("failed-create");
        address predicted = caller.predictAssetAddress(address(caller), failedSalt);
        bytes[] memory initCalls = new bytes[](1);
        initCalls[0] = hex"deadbeef";

        vm.expectRevert(abi.encodeWithSelector(IB20Factory.InitCallFailed.selector, uint256(0)));
        caller.createAssetWithInit(failedSalt, "Failed", "FAIL", address(this), initCalls);

        assertEq(predicted.code.length, 0, "failed create left marker");
        assertTrue(!IB20Factory(FACTORY).isB20Initialized(predicted), "failed create left storage");
        _assertUninitialized(predicted);

        IB20 token = IB20(caller.createAsset(keccak256("failed-mint"), "Rollback", "RBK", address(this)));
        token.grantRole(token.MINT_ROLE(), address(rollback));
        vm.expectRevert(B20Rollback.ForcedRollback.selector);
        rollback.mintThenRevert(token, ALICE, 10e18);

        assertEq(token.totalSupply(), 0, "reverted mint changed supply");
        assertEq(token.balanceOf(ALICE), 0, "reverted mint changed balance");
    }

    function testSnapshotRestoresDynamicStateAndGenesisBaseline() public {
        uint256 snapshotId = vm.snapshotState();
        address predicted = caller.predictAssetAddress(address(caller), SALT);
        IB20 token = IB20(caller.createAsset(SALT, "Snapshot", "SNP", address(this)));
        token.grantRole(token.MINT_ROLE(), address(this));
        token.mint(ALICE, 12e18);

        assertEq(predicted.code, hex"ef", "snapshot token marker missing");
        assertEq(token.balanceOf(ALICE), 12e18, "snapshot token balance missing");
        assertTrue(vm.revertToState(snapshotId), "snapshot revert failed");

        assertEq(predicted.code.length, 0, "snapshot retained dynamic marker");
        assertTrue(!IB20Factory(FACTORY).isB20Initialized(predicted), "snapshot retained token state");
        _assertUninitialized(predicted);
        assertEq(FACTORY.code, hex"ef", "snapshot removed factory baseline");
        assertEq(ACTIVATION_REGISTRY.code, hex"ef", "snapshot removed activation baseline");
        assertEq(POLICY_REGISTRY.code, hex"ef", "snapshot removed policy baseline");
        assertTrue(
            IActivationRegistry(ACTIVATION_REGISTRY).isActivated(ASSET_FEATURE), "snapshot removed activation state"
        );

        address recreated = caller.createAsset(SALT, "Snapshot", "SNP", address(this));
        assertEq(recreated, predicted, "snapshot did not restore factory state");
        assertEq(IB20(recreated).totalSupply(), 0, "snapshot retained token storage");
    }

    function _assertUninitialized(address token) internal {
        (bool ok, bytes memory data) = token.call(abi.encodeWithSelector(IB20.name.selector));
        assertTrue(!ok, "uninitialized B20 call succeeded");
        assertEq(data.length, 0, "uninitialized B20 returned typed data");
    }
}
