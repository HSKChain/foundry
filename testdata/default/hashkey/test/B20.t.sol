// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import {Test} from "forge-std/Test.sol";
import {B20Caller, IB20Factory, IB20} from "../src/B20.sol";

contract B20AssetTest is Test {
    B20Caller caller;
    address constant FACTORY = 0xB20f000000000000000000000000000000000000;
    address constant ACTIVATION_REGISTRY = 0x8453000000000000000000000000000000000001;
    address constant POLICY_REGISTRY = 0x8453000000000000000000000000000000000002;
    bytes32 constant SALT = bytes32(uint256(0xC0FFEE));

    function setUp() public {
        caller = new B20Caller();
    }

    function testSingletonMarkersPresent() public view {
        assertEq(FACTORY.code, hex"ef", "B20Factory marker missing");
        assertEq(ACTIVATION_REGISTRY.code, hex"ef", "ActivationRegistry marker missing");
        assertEq(POLICY_REGISTRY.code, hex"ef", "PolicyRegistry marker missing");
    }

    function testDeterministicCreateAndMarker() public {
        address predicted = caller.predictAssetAddress(address(caller), SALT);
        address token = caller.createAsset(SALT, "TestAsset", "TST", address(this));

        assertEq(token, predicted, "deterministic address mismatch");
        assertEq(uint8(uint160(token) >> 152), 0xb2, "token address must have B20 prefix");
        assertEq(token.code, hex"ef", "dynamic token marker missing");
        assertTrue(IB20Factory(FACTORY).isB20Initialized(token), "token must be initialized");
    }

    function testMintAndTransfer() public {
        address token = caller.createAsset(
            keccak256("state-op"),
            "StateAsset",
            "STA",
            address(this)
        );
        IB20 b20 = IB20(token);
        b20.grantRole(b20.MINT_ROLE(), address(this));

        b20.mint(address(0xBEEF), 1000e18);
        assertEq(b20.balanceOf(address(0xBEEF)), 1000e18, "mint balance mismatch");
        assertEq(b20.totalSupply(), 1000e18, "total supply mismatch");

        vm.prank(address(0xBEEF));
        assertTrue(b20.transfer(address(0xCAFE), 400e18), "transfer failed");
        assertEq(b20.balanceOf(address(0xBEEF)), 600e18, "sender balance mismatch");
        assertEq(b20.balanceOf(address(0xCAFE)), 400e18, "receiver balance mismatch");
    }
}
