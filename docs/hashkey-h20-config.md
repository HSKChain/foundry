# HashKey H20 configuration reference

The `hashkey` network selector enables the opt-in HashKey H20 standalone-local profile. It resolves
to the Optimism EVM family plus the Beryl H20 v1 native precompiles.

## Build capability

The selector exists only in binaries compiled with the `hashkey` Cargo feature. Official HSKChain
artifacts and the root `make build` target include it. A minimal source build is:

```sh
cargo build --locked \
  -p forge -p cast -p anvil -p chisel \
  --features hashkey
```

Cargo keeps the standard target names. To build the checkout with the feature and install the
namespaced commands, run `hsk-foundryup --path "$PWD"` from the repository root.

A binary built without this feature rejects `hashkey` as an unknown `--network` value. Enabling the
feature only adds the capability; it does not activate H20 unless the runtime selector is also set.

## Setting

| Key | Type | Default | Environment variable |
| --- | --- | --- | --- |
| `network` | `ethereum`, `optimism`, `tempo`, or `hashkey` | Ethereum semantics | None |

Command-line form:

```sh
hsk-forge test --network hashkey
hsk-anvil --network hashkey
hsk-cast run --network hashkey --rpc-url http://127.0.0.1:8545 TRANSACTION_HASH
hsk-chisel --network hashkey
```

`hsk-cast call` and `hsk-cast send` execute through the connected RPC node and therefore do not take the
selector. Point them at a HashKey Anvil RPC. Use `--network hashkey` for Cast commands such as
`hsk-cast run` that replay or execute transactions in Cast's local EVM.

Project configuration:

```toml
[profile.default]
network = "hashkey"
```

## Precedence and resolution

1. An explicitly supplied `--network` value overrides `foundry.toml` for that command.
2. Otherwise, `network` is loaded from the selected Foundry profile.
3. Without either selector, normal Ethereum or chain-derived behavior applies. A chain ID never
   implicitly enables HashKey H20; HashKey selection is always explicit.

`--network` conflicts with the legacy `--optimism`, `--tempo`, and `--celo` flags. The canonical
serialized configuration is always `network = "..."`.

When selected, `hashkey` resolves once to an immutable runtime profile with:

- Optimism EVM execution semantics.
- H20 activation time `0`, active inclusively at every standalone-local timestamp.
- Development activation admin `0xCB00000000000000000000000000000000000000`.
- `H20Asset`, `H20Stablecoin`, and `PolicyRegistry` active in fresh standalone state.
- Marker bytecode `0xef` on the Factory, ActivationRegistry, and PolicyRegistry singleton accounts.

These values are deterministic development fixtures, not HashKey mainnet or testnet parameters.
They are seeded only for fresh standalone state. `--fork-url` preserves remote code and storage and
does not apply the local H20 seed, so no production or historical-fidelity guarantee is implied.

See [HashKey H20 local simulation](./hashkey-h20.md) for workflows and the complete support boundary.
