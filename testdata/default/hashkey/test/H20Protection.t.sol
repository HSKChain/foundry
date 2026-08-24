// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import {Test} from "forge-std/Test.sol";
import {H20Caller, IH20} from "../src/H20.sol";

contract H20ProtectionTest is Test {
    H20Caller caller;

    address constant FACTORY = 0x0177FF0000000000000000000000000000000000;
    address constant ACTIVATION_REGISTRY = 0x0177FF0000000000000000000000000000000001;
    address constant POLICY_REGISTRY = 0x0177FF0000000000000000000000000000000002;
    bytes32 constant ASSET_FEATURE = keccak256("hsk.h20_asset");

    function setUp() public {
        caller = new H20Caller();
    }

    function testLoadRemainsAvailableForProtectedState() public {
        bytes32 namespaceHash = keccak256("base.activation_registry");
        bytes32 namespaceRoot = keccak256(abi.encode(uint256(namespaceHash) - 1)) & ~bytes32(uint256(0xff));
        bytes32 featureSlot = keccak256(abi.encode(ASSET_FEATURE, namespaceRoot));
        address token = caller.createAsset(keccak256("load"), "Loadable", "LOAD", address(this));

        assertEq(vm.load(ACTIVATION_REGISTRY, featureSlot), bytes32(uint256(1)), "singleton load failed");
        vm.load(token, bytes32(0));
    }

    function testStoreAndEtchRejectFixedSingletons() public {
        address[3] memory singletons = [FACTORY, ACTIVATION_REGISTRY, POLICY_REGISTRY];

        for (uint256 i; i < singletons.length; i++) {
            vm.expectRevert();
            this.store(singletons[i], bytes32(0), bytes32(uint256(1)));

            vm.expectRevert();
            this.etch(singletons[i], hex"00");

            assertEq(singletons[i].code, hex"ef", "singleton marker changed");
        }
    }

    function testStoreAndEtchRejectInitializedDynamicToken() public {
        address token = caller.createAsset(keccak256("protected"), "Protected", "PRT", address(this));

        vm.expectRevert();
        this.store(token, bytes32(0), bytes32(uint256(1)));

        vm.expectRevert();
        this.etch(token, hex"00");

        assertEq(token.code, hex"ef", "dynamic marker changed");
        assertEq(IH20(token).name(), "Protected", "dynamic storage changed");
    }

    function testUninitializedDynamicAddressRemainsMutable() public {
        bytes32 salt = keccak256("uninitialized");
        address token = caller.predictAssetAddress(address(caller), salt);
        bytes32 slot = bytes32(uint256(7));
        bytes32 value = bytes32(uint256(9));

        assertEq(token.code.length, 0, "predicted token unexpectedly initialized");
        vm.store(token, slot, value);
        assertEq(vm.load(token, slot), value, "uninitialized store failed");
        vm.etch(token, hex"6000");
        assertEq(token.code, hex"6000", "uninitialized etch failed");
    }

    function testUnrelatedAddressRemainsMutable() public {
        address target = address(0x1234);
        bytes32 slot = bytes32(uint256(3));
        bytes32 value = bytes32(uint256(4));

        vm.store(target, slot, value);
        vm.etch(target, hex"6001");

        assertEq(vm.load(target, slot), value, "ordinary store failed");
        assertEq(target.code, hex"6001", "ordinary etch failed");
    }

    function store(address target, bytes32 slot, bytes32 value) external {
        vm.store(target, slot, value);
    }

    function etch(address target, bytes calldata code) external {
        vm.etch(target, code);
    }
}
