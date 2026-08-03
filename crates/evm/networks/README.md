# Custom EVM Networks

The evm-networks crate defines custom network features that are shared across Foundry's tooling (`anvil`, `forge` and 
`cast`). Currently, it supports custom precompiles, with planned support for custom transaction types.

## Adding a Custom Network
To add configuration support for a custom network (e.g. `my_network`), add a new field to the `NetworkConfigs` struct:

```rust
    /// Enable my custom network features.
    #[arg(help_heading = "Networks", long)]
    #[serde(default)]
    pub my_network: bool,
```

This automatically enables:
- `my_network = true` in foundry.toml
- `--my-network` anvil CLI flag
```
Networks:
      --my-network
          Enable my custom network features

```

If you'd like network features to be enabled automatically based on the chain ID, register the
chain in `NetworkConfigs::from_known_chain_id` so command profile resolution
(`CommandProfileResolution` in `foundry-evm-core`) can select the profile from a chain hint or
fork identity:

```rust
impl NetworkConfigs {
    pub fn from_known_chain_id(chain_id: u64) -> Option<Self> {
        // Returns `Some(configs)` for your known chain identity.
    }
}
```

## Adding a custom precompile

- Create a module for your network-specific logic, e.g., `my_network/transfer`.
- Implement the precompile logic as a function that accepts a `PrecompileInput` containing execution context and hooks for 
interacting with EVM state, and returns a `PrecompileResult`:

```rust
pub fn custom_precompile(
  input: alloy_evm::precompiles::PrecompileInput<'_>
) -> revm::precompile::PrecompileResult {
  // Your logic here
}
```

- Enable the precompile in the `NetworkConfigs` implementation by conditionally applying it to an address:

```rust
if self.my_network {
    precompiles.apply_precompile(&MY_NETWORK_TRANSFER_ADDRESS, move |_| {
        Some(my_network::transfer::custom_precompile())
    });
}
```