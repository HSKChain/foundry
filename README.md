<div align="center">
  <img src=".github/assets/banner.png" alt="Foundry banner" />

&nbsp;

[![Github Actions][gha-badge]][gha-url] [![Telegram Chat][tg-badge]][tg-url] [![Telegram Support][tg-support-badge]][tg-support-url]

[gha-badge]: https://img.shields.io/github/actions/workflow/status/foundry-rs/foundry/test.yml?branch=master&style=flat-square
[gha-url]: https://github.com/foundry-rs/foundry/actions
[tg-badge]: https://img.shields.io/endpoint?color=neon&logo=telegram&label=chat&style=flat-square&url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Ffoundry_rs
[tg-url]: https://t.me/foundry_rs
[tg-support-badge]: https://img.shields.io/endpoint?color=neon&logo=telegram&label=support&style=flat-square&url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Ffoundry_support
[tg-support-url]: https://t.me/foundry_support

**[Install](https://getfoundry.sh/getting-started/installation)**
| [Docs][foundry-docs]
| [Benchmarks](https://www.getfoundry.sh/benchmarks)
| [Developer Guidelines](./docs/dev/README.md)
| [Contributing](./CONTRIBUTING.md)
| [Crate Docs](https://foundry-rs.github.io/foundry)

</div>

---

Blazing fast, portable and modular toolkit for Ethereum application development, written in Rust.

- [**Forge**](https://getfoundry.sh/forge) — Build, test, fuzz, debug and deploy Solidity contracts.
- [**Cast**](https://getfoundry.sh/cast) — Swiss Army knife for interacting with EVM smart contracts, sending transactions and getting chain data.
- [**Anvil**](https://getfoundry.sh/anvil) — Fast local Ethereum development node.
- [**Chisel**](https://getfoundry.sh/chisel) — Fast, utilitarian and verbose Solidity REPL.

![Demo](.github/assets/demo.gif)

## Installation

```sh
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

See the [installation guide](https://getfoundry.sh/getting-started/installation) for more details.

To verify a downloaded release archive or container image, see [Verifying Releases](./SECURITY.md#verifying-releases).

### HashKey B20 local profile

HSKChain release archives keep Foundry's standard `forge`, `cast`, `anvil`, and `chisel` binary
names. The HSKChain installer exposes them through namespaced wrappers so they can coexist with a
stock Foundry installation:

```sh
curl -L https://raw.githubusercontent.com/HSKChain/foundry/HEAD/foundryup/install | bash
hsk-foundryup --install v1.7.1-hsk-b20
```

This installs `hsk-forge`, `hsk-cast`, `hsk-anvil`, and `hsk-chisel` without replacing
`foundryup` or the stock commands. The same release can be built from its tag and registered under
the namespaced commands:

```sh
git clone https://github.com/HSKChain/foundry.git foundry-hsk
cd foundry-hsk
git checkout v1.7.1-hsk-b20
hsk-foundryup --path "$PWD"
```

Select the standalone local profile explicitly:

```sh
hsk-anvil --network hashkey
hsk-forge test --network hashkey
hsk-cast call --rpc-url http://127.0.0.1:8545 ADDRESS "function()"
hsk-cast run --network hashkey --rpc-url http://127.0.0.1:8545 TRANSACTION_HASH
hsk-chisel --network hashkey
```

Or make it the project default in `foundry.toml`:

```toml
[profile.default]
network = "hashkey"
```

This profile provides deterministic **local-development** B20 state. Its activation time, admin,
and feature state are not HashKey mainnet or testnet parameters. See the
[HashKey B20 local simulation guide](./docs/hashkey-b20.md) and
[configuration reference](./docs/hashkey-b20-config.md) before using it.

## Getting Started

Initialize a new project, build and test:

```sh
forge init counter && cd counter
forge build
forge test
```

Interact with a live network:

```sh
cast block-number --rpc-url https://eth.merkle.io
cast balance vitalik.eth --ether --rpc-url https://eth.merkle.io
```

Fork mainnet locally:

```sh
anvil --fork-url https://eth.merkle.io
```

Read the [Foundry Docs][foundry-docs] to learn more.

## Contributing

Contributions are welcome and highly appreciated. To get started, check out the [contributing guidelines](./CONTRIBUTING.md).

Join our [Telegram][tg-url] to chat about the development of Foundry.

## Support

Having trouble? Check the [Foundry Docs][foundry-docs], join the [support Telegram][tg-support-url], or [open an issue](https://github.com/foundry-rs/foundry/issues/new).

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in these crates by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
</sub>

[foundry-docs]: https://getfoundry.sh
