# HashKey B20 local simulation

The HSKChain Foundry release provides a deterministic, opt-in environment for compiling Solidity
callers and exercising Beryl B20 v1 native precompiles with Forge, Anvil, Cast, and Chisel. It is a
standalone local-development profile, not a model of current or historical HashKey production state.

## Support boundary

Supported:

- Fresh standalone Forge execution selected with `--network hashkey` or `network = "hashkey"`.
- Fresh standalone HashKey Anvil nodes and Cast clients connected to those nodes.
- Chisel sessions using the same profile.
- Deterministic B20 Factory, Asset, Stablecoin, ActivationRegistry, and PolicyRegistry behavior.
- Ordinary Foundry and Anvil snapshot/revert behavior for B20 marker code and storage.

Not guaranteed:

- HashKey mainnet or testnet activation timestamps, governance admins, or rollout state.
- Automatic profile selection from a chain ID or RPC endpoint.
- Production or historical fidelity for remote RPC calls and forks.
- Local seed state on a fork. Fork mode deliberately preserves the remote block's code and storage.

## Install and select the profile

HSKChain releases use the ordinary `forge`, `cast`, `anvil`, and `chisel` names and an identifiable
tag such as `v1.7.1-hsk-b20`. Installation and locked source-build commands are in the
[README](../README.md#hashkey-b20-local-profile).

Select HashKey for one command:

```sh
forge test --network hashkey
```

Or select it for the project:

```toml
[profile.default]
network = "hashkey"
```

An explicit CLI selector takes precedence over the project setting. The binary must also have been
built with the `hashkey` Cargo feature. See the [configuration reference](./hashkey-b20-config.md).

## Deterministic development state

Fresh standalone execution uses the following local fixture:

| Item | Local value |
| --- | --- |
| B20 activation time | `0` |
| Development activation admin | `0xCB00000000000000000000000000000000000000` |
| B20 Factory | `0xB20F000000000000000000000000000000000000` |
| ActivationRegistry | `0x8453000000000000000000000000000000000001` |
| PolicyRegistry | `0x8453000000000000000000000000000000000002` |
| Initially active features | `B20Asset`, `B20Stablecoin`, `PolicyRegistry` |
| Singleton marker bytecode | `0xef` |

The feature identifiers are `keccak256("base.b20_asset")`,
`keccak256("base.b20_stablecoin")`, and `keccak256("base.policy_registry")`.

The three singleton markers and activation slots are initialized once at the standalone backend or
genesis boundary. They are not replayed whenever a new EVM is created. Dynamic Asset and Stablecoin
addresses start empty and receive their `0xef` marker and storage atomically only when the Factory
successfully creates them.

## Solidity callers and Forge

B20 is implemented as native Rust/revm precompiles. Solidity projects compile interfaces and callers,
not the B20 implementation itself. A minimal activation query is:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface IActivationRegistry {
    function isActivated(bytes32 feature) external view returns (bool);
}

contract B20Status {
    IActivationRegistry constant REGISTRY =
        IActivationRegistry(0x8453000000000000000000000000000000000001);

    function assetActive() external view returns (bool) {
        return REGISTRY.isActivated(keccak256("base.b20_asset"));
    }
}
```

Run tests with:

```sh
forge test --network hashkey -vvvv
```

The development activation admin is useful for lifecycle tests. Use `vm.prank` rather than changing
the deterministic fixture:

```solidity
address constant DEV_ADMIN = 0xCB00000000000000000000000000000000000000;
bytes32 constant ASSET_FEATURE = keccak256("base.b20_asset");

vm.prank(DEV_ADMIN);
IActivationRegistry(0x8453000000000000000000000000000000000001)
    .deactivate(ASSET_FEATURE);
```

`vm.load` remains available for read-only inspection. Mutation cheatcodes such as `vm.store` and
`vm.etch` reject the three singleton accounts and initialized dynamic B20 token addresses. An
uninitialized address that merely has the B20-shaped prefix is not protected.

## Anvil and Cast

Start a fresh standalone node:

```sh
anvil --network hashkey
```

Inspect the local activation state:

```sh
ASSET_FEATURE=$(cast keccak "base.b20_asset")
cast call \
  --rpc-url http://127.0.0.1:8545 \
  0x8453000000000000000000000000000000000001 \
  "isActivated(bytes32)(bool)" "$ASSET_FEATURE"
```

To test admin-only operations, impersonate the development admin on the local node:

```sh
DEV_ADMIN=0xCB00000000000000000000000000000000000000
RPC=http://127.0.0.1:8545

cast rpc --rpc-url "$RPC" anvil_impersonateAccount "$DEV_ADMIN"
cast send \
  --rpc-url "$RPC" \
  --unlocked --from "$DEV_ADMIN" \
  0x8453000000000000000000000000000000000001 \
  "deactivate(bytes32)" "$ASSET_FEATURE"
cast rpc --rpc-url "$RPC" anvil_stopImpersonatingAccount "$DEV_ADMIN"
```

RPC-backed `cast call` and `cast send` use the Anvil node's execution profile. For local replay and
debug execution inside Cast itself, select the profile explicitly:

```sh
cast run --network hashkey --rpc-url "$RPC" TRANSACTION_HASH
```

Anvil's normal `evm_snapshot` and `evm_revert` operations include B20 code and storage. Reverting a
post-genesis dynamic token creation removes its marker and token storage, while the singleton marker
and initial activation baseline remain present.

Starting Anvil with `--fork-url` changes the state contract: the fork uses the remote block as its
baseline and does not inject the deterministic local markers, feature slots, or admin.

## Chisel

Start a HashKey-aware REPL:

```sh
chisel --network hashkey --offline -vvvv
```

Define the same Solidity interfaces used by a project, then call the Factory or registries normally.
The session keeps the resolved HashKey profile and state across REPL rebuilds. `-vvvv` shows stable
labels such as `B20Factory`, `B20Asset`, and `B20Stablecoin` in traces.

## Traces and decoding

Forge, Anvil debug RPCs, Cast replay/debug commands, and Chisel decode B20 calls only when all of the
following match:

- The resolved profile is HashKey.
- B20 is active at the EVM creation timestamp.
- The called address is the relevant singleton or an initialized canonical dynamic token.

Malformed or incompatible calldata remains raw instead of being assigned a misleading global
selector match. This keeps ordinary Ethereum and Optimism traces unchanged.

## Release identity

An HSK release is reproducible with `cargo build --locked`, includes the `hashkey` feature in the
standard release and Docker workflows, and publishes machine-readable compatibility metadata. The
metadata records the release tag and Foundry commit together with the exact HashKey optimism, Tempo,
Reth, OP-revm, and OP Alloy revisions resolved by the release lockfile.
