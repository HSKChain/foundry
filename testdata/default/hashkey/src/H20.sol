// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IH20Factory {
    enum H20Variant {
        ASSET,
        STABLECOIN
    }

    struct H20AssetCreateParams {
        uint8 version;
        string name;
        string symbol;
        address initialAdmin;
        uint8 decimals;
    }

    struct H20StablecoinCreateParams {
        uint8 version;
        string name;
        string symbol;
        address initialAdmin;
        string currency;
    }

    error InitCallFailed(uint256 index);

    function createH20(H20Variant variant, bytes32 salt, bytes calldata params, bytes[] calldata initCalls)
        external
        returns (address token);

    function getH20Address(H20Variant variant, address sender, bytes32 salt) external view returns (address);

    function isH20Initialized(address token) external view returns (bool);
}

interface IH20 {
    error AccessControlUnauthorizedAccount(address account, bytes32 neededRole);

    function name() external view returns (string memory);
    function decimals() external view returns (uint8);
    function totalSupply() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
    function mint(address to, uint256 amount) external;
    function MINT_ROLE() external view returns (bytes32);
    function grantRole(bytes32 role, address account) external;
}

interface IH20Stablecoin is IH20 {
    function currency() external view returns (string memory);
}

interface IActivationRegistry {
    error AlreadyActivated(bytes32 feature);
    error FeatureNotActivated(bytes32 feature);

    function admin() external view returns (address);
    function isActivated(bytes32 feature) external view returns (bool);
    function checkActivated(bytes32 feature) external view;
    function activate(bytes32 feature) external;
    function deactivate(bytes32 feature) external;
}

interface IPolicyRegistry {
    enum PolicyType {
        BLOCKLIST,
        ALLOWLIST
    }

    error Unauthorized();

    function createPolicy(address admin, PolicyType policyType) external returns (uint64);
    function updateAllowlist(uint64 policyId, bool allowed, address[] calldata accounts) external;
    function isAuthorized(uint64 policyId, address account) external view returns (bool);
    function policyExists(uint64 policyId) external view returns (bool);
    function policyAdmin(uint64 policyId) external view returns (address);
}

contract H20Caller {
    address constant FACTORY = 0x0177FF0000000000000000000000000000000000;

    function createAsset(bytes32 salt, string memory name, string memory symbol, address admin)
        external
        returns (address token)
    {
        bytes[] memory noInitCalls = new bytes[](0);
        return createAssetWithInit(salt, name, symbol, admin, noInitCalls);
    }

    function createAssetWithInit(
        bytes32 salt,
        string memory name,
        string memory symbol,
        address admin,
        bytes[] memory initCalls
    ) public returns (address token) {
        bytes memory params = abi.encode(
            IH20Factory.H20AssetCreateParams({
                version: 1, name: name, symbol: symbol, initialAdmin: admin, decimals: 18
            })
        );
        token = IH20Factory(FACTORY).createH20(IH20Factory.H20Variant.ASSET, salt, params, initCalls);
    }

    function createStablecoin(
        bytes32 salt,
        string memory name,
        string memory symbol,
        address admin,
        string memory currency
    ) external returns (address token) {
        bytes memory params = abi.encode(
            IH20Factory.H20StablecoinCreateParams({
                version: 1, name: name, symbol: symbol, initialAdmin: admin, currency: currency
            })
        );
        bytes[] memory noInitCalls = new bytes[](0);
        token = IH20Factory(FACTORY).createH20(IH20Factory.H20Variant.STABLECOIN, salt, params, noInitCalls);
    }

    function predictAssetAddress(address creator, bytes32 salt) external view returns (address) {
        return IH20Factory(FACTORY).getH20Address(IH20Factory.H20Variant.ASSET, creator, salt);
    }
}

contract H20Rollback {
    error ForcedRollback();

    function mintThenRevert(IH20 token, address to, uint256 amount) external {
        token.mint(to, amount);
        revert ForcedRollback();
    }
}
