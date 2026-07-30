// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IB20Factory {
    enum B20Variant {
        ASSET,
        STABLECOIN
    }

    struct B20AssetCreateParams {
        uint8 version;
        string name;
        string symbol;
        address initialAdmin;
        uint8 decimals;
    }

    function createB20(
        B20Variant variant,
        bytes32 salt,
        bytes calldata params,
        bytes[] calldata initCalls
    ) external returns (address token);

    function getB20Address(B20Variant variant, address sender, bytes32 salt)
        external
        view
        returns (address);

    function isB20Initialized(address token) external view returns (bool);
}

interface IB20 {
    function totalSupply() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
    function mint(address to, uint256 amount) external;
    function MINT_ROLE() external view returns (bytes32);
    function grantRole(bytes32 role, address account) external;
}

contract B20Caller {
    address constant FACTORY = 0xB20f000000000000000000000000000000000000;

    function createAsset(bytes32 salt, string memory name, string memory symbol, address admin)
        external
        returns (address token)
    {
        bytes memory params = abi.encode(
            IB20Factory.B20AssetCreateParams({
                version: 1,
                name: name,
                symbol: symbol,
                initialAdmin: admin,
                decimals: 18
            })
        );
        bytes[] memory noInitCalls = new bytes[](0);
        token = IB20Factory(FACTORY).createB20(IB20Factory.B20Variant.ASSET, salt, params, noInitCalls);
    }

    function predictAssetAddress(address creator, bytes32 salt) external view returns (address) {
        return IB20Factory(FACTORY).getB20Address(IB20Factory.B20Variant.ASSET, creator, salt);
    }
}
