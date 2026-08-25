# HashKey H20 local simulation

The HSKChain Foundry release provides a deterministic, opt-in environment for compiling Solidity
callers and exercising Beryl H20 v1 native precompiles with Forge, Anvil, Cast, and Chisel. It is a
standalone local-development profile, not a model of current or historical HashKey production state.

## Support boundary

Supported:

- Fresh standalone Forge execution selected with `--network hashkey` or `network = "hashkey"`.
- Fresh standalone HashKey Anvil nodes and Cast clients connected to those nodes.
- Chisel sessions using the same profile.
- Deterministic H20 Factory, Asset, Stablecoin, ActivationRegistry, and PolicyRegistry behavior.
- Ordinary Foundry and Anvil snapshot/revert behavior for H20 marker code and storage.

Not guaranteed:

- HashKey mainnet or testnet activation timestamps, governance admins, or rollout state.
- Automatic profile selection from a chain ID or RPC endpoint.
- Production or historical fidelity for remote RPC calls and forks.
- Local seed state on a fork. Fork mode deliberately preserves the remote block's code and storage.

## Install and select the profile

HSKChain release archives retain the ordinary Foundry binary names. `hsk-foundryup` installs
namespaced `hsk-forge`, `hsk-cast`, `hsk-anvil`, and `hsk-chisel` wrappers without replacing a
stock Foundry installation. Installation and source-build commands are in the
[README](../README.md#hashkey-h20-local-profile).

Select HashKey for one command:

```sh
hsk-forge test --network hashkey
```

Or select it for the project:

```toml
[profile.default]
network = "hashkey"
```

An explicit CLI selector takes precedence over the project setting. The binary must also have been
built with the `hashkey` Cargo feature. See the [configuration reference](./hashkey-h20-config.md).

## Deterministic development state

Fresh standalone execution uses the following local fixture:

| Item | Local value |
| --- | --- |
| H20 activation time | `0` |
| Development activation admin | `0xCB00000000000000000000000000000000000000` |
| H20 Factory | `0x0177FF0000000000000000000000000000000000` |
| ActivationRegistry | `0x0177FF0000000000000000000000000000000001` |
| PolicyRegistry | `0x0177FF0000000000000000000000000000000002` |
| Initially active features | `H20Asset`, `H20Stablecoin`, `PolicyRegistry` |
| Singleton marker bytecode | `0xef` |

The feature identifiers are `keccak256("hsk.h20_asset")`,
`keccak256("hsk.h20_stablecoin")`, and `keccak256("hsk.policy_registry")`.

The three singleton markers and activation slots are initialized once at the standalone backend or
genesis boundary. They are not replayed whenever a new EVM is created. Dynamic Asset and Stablecoin
addresses start empty and receive their `0xef` marker and storage atomically only when the Factory
successfully creates them.

## Solidity callers and Forge

H20 is implemented as native Rust/revm precompiles. Solidity projects compile interfaces and callers,
not the H20 implementation itself. A minimal activation query is:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface IActivationRegistry {
    function isActivated(bytes32 feature) external view returns (bool);
}

contract H20Status {
    IActivationRegistry constant REGISTRY =
        IActivationRegistry(0x0177FF0000000000000000000000000000000001);

    function assetActive() external view returns (bool) {
        return REGISTRY.isActivated(keccak256("hsk.h20_asset"));
    }
}
```

Run tests with:

```sh
hsk-forge test --network hashkey -vvvv
```

The development activation admin is useful for lifecycle tests. Use `vm.prank` rather than changing
the deterministic fixture:

```solidity
address constant DEV_ADMIN = 0xCB00000000000000000000000000000000000000;
bytes32 constant ASSET_FEATURE = keccak256("hsk.h20_asset");

vm.prank(DEV_ADMIN);
IActivationRegistry(0x0177FF0000000000000000000000000000000001)
    .deactivate(ASSET_FEATURE);
```

`vm.load` remains available for read-only inspection. Mutation cheatcodes such as `vm.store` and
`vm.etch` reject the three singleton accounts and initialized dynamic H20 token addresses. An
uninitialized address that merely has the H20-shaped prefix is not protected.

## Anvil and Cast

Start a fresh standalone node:

```sh
hsk-anvil --network hashkey
```

Inspect the local activation state:

```sh
ASSET_FEATURE=$(hsk-cast keccak "hsk.h20_asset")
hsk-cast call \
  --rpc-url http://127.0.0.1:8545 \
  0x0177FF0000000000000000000000000000000001 \
  "isActivated(bytes32)(bool)" "$ASSET_FEATURE"
```

To test admin-only operations, impersonate the development admin on the local node:

```sh
DEV_ADMIN=0xCB00000000000000000000000000000000000000
RPC=http://127.0.0.1:8545

hsk-cast rpc --rpc-url "$RPC" anvil_impersonateAccount "$DEV_ADMIN"
hsk-cast send \
  --rpc-url "$RPC" \
  --unlocked --from "$DEV_ADMIN" \
  0x0177FF0000000000000000000000000000000001 \
  "deactivate(bytes32)" "$ASSET_FEATURE"
hsk-cast rpc --rpc-url "$RPC" anvil_stopImpersonatingAccount "$DEV_ADMIN"
```

RPC-backed `hsk-cast call` and `hsk-cast send` use the Anvil node's execution profile. For local replay and
debug execution inside Cast itself, select the profile explicitly:

```sh
hsk-cast run --network hashkey --rpc-url "$RPC" TRANSACTION_HASH
```

Anvil's normal `evm_snapshot` and `evm_revert` operations include H20 code and storage. Reverting a
post-genesis dynamic token creation removes its marker and token storage, while the singleton marker
and initial activation baseline remain present.

Starting Anvil with `--fork-url` changes the state contract: the fork uses the remote block as its
baseline and does not inject the deterministic local markers, feature slots, or admin.

## Chisel

Start a HashKey-aware REPL:

```sh
hsk-chisel --network hashkey --offline -vvvv
```

Define the same Solidity interfaces used by a project, then call the Factory or registries normally.
The session keeps the resolved HashKey profile and state across REPL rebuilds. `-vvvv` shows stable
labels such as `H20Factory`, `H20Asset`, and `H20Stablecoin` in traces.

## Traces and decoding

Forge, Anvil debug RPCs, Cast replay/debug commands, and Chisel decode H20 calls only when all of the
following match:

- The resolved profile is HashKey.
- H20 is active at the EVM creation timestamp.
- The called address is the relevant singleton or an initialized canonical dynamic token.

Malformed or incompatible calldata remains raw instead of being assigned a misleading global
selector match. This keeps ordinary Ethereum and Optimism traces unchanged.

## Release identity

An HSK release is reproducible with `cargo build --locked`, includes the `hashkey` feature in the
standard release and Docker workflows, and publishes machine-readable compatibility metadata. The
metadata records the release tag and Foundry commit together with the exact HashKey optimism, Tempo,
Reth, OP-revm, and OP Alloy revisions resolved by the release lockfile.
