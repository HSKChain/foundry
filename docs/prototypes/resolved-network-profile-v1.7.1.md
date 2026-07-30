# v1.7.1 `ResolvedNetworkProfile` seam 原型

> 状态：issue #28 的设计原型。本文回答 constructor/transport 问题；不实现 production
> runtime seam。

## 问题与结论

需要回答的问题是：Foundry v1.7.1 如何只 resolve 一次 immutable network profile，并将同一个
值传过全部 EVM construction path，同时避免在 Forge、Script、Anvil、Cast、Chisel、Verify
中散布 `network == hashkey` 判断。

适配 v1.7.1 的结论是：

1. `NetworkConfigs` 只作为尚未 resolve 的 CLI/TOML/provider input。
2. 每个 command boundary 先把 command-local selector（例如 Cast Tempo transaction args、
   Script fee token）fold 进 `NetworkConfigs`，再完成所需的 fork network inference，最后只调用
   一次 `NetworkConfigs::resolve()`。
3. generic EVM type 只由 `ResolvedNetworkProfile::evm_family()` dispatch。
4. 将同一个 `ResolvedNetworkProfile` 值复制进 `EvmOpts` helper、`CreateFork`、`Backend`、
   `InspectorStack` 和 command-owned runtime config。
5. precompile 等 network behavior 只由 profile 与显式
   `NetworkExecutionContext { chain_id, timestamp }` 投影。

`ResolvedNetworkProfile` 是 `Copy` type。“同一个 profile instance”在这里表示一个 resolved
value 被原样复制，不表示 pointer identity。command boundary 之后，任何 transport path 再调用
`resolve()`、`with_chain_id()` 或构造 `ResolvedNetworkProfile::default()` 都属于 seam 丢失。

该结论保留 `775946811` 已证明的 ownership 与 `a56f34ac1` 的 transport audit 意图，但按
`v1.7.1-hsk-b20@28ab3ace5` 实际存在的 constructor 重新映射。

## Public type seam

production type 应位于 `crates/evm/networks/src/lib.rs`。v1.7.1 所需的最小定义是：

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvmFamily {
    #[default]
    Ethereum,
    Optimism,
    Tempo,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkExecutionContext {
    pub chain_id: ChainId,
    pub timestamp: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkStatePlan {
    #[default]
    None,
    Tempo,
    #[cfg(feature = "hashkey")]
    HashKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolvedNetworkProfile {
    family: EvmFamily,
    celo: bool,
    bypass_prevrandao: bool,
    #[cfg(feature = "hashkey")]
    hashkey: bool,
    #[cfg(feature = "hashkey")]
    b20_activation_time: Option<u64>,
    #[cfg(feature = "hashkey")]
    b20_activation_admin: Option<Address>,
}
```

成对的 B20 fields 有意沿用已审计 seam，而不是存放可由用户自由拼装的 config。
`B20Config::new(...)` 继续作为 validator 与 projection API。由于 pinned
`B20Config::new` 不是 `const`，保留两个 fields 还能让 `NetworkConfigs::resolve()` 继续是
`const fn`。

必须提供以下 projections：

```rust
impl ResolvedNetworkProfile {
    pub const fn evm_family(self) -> EvmFamily;
    pub const fn name(self) -> &'static str;
    pub const fn is_celo(self) -> bool;
    pub const fn is_tempo(self) -> bool;
    pub const fn is_optimism(self) -> bool;
    #[cfg(feature = "hashkey")]
    pub const fn is_hashkey(self) -> bool;
    #[cfg(feature = "hashkey")]
    pub fn b20_config(self) -> B20Config;
    pub const fn state_plan(self) -> NetworkStatePlan;
    pub fn base_fee_params(self, timestamp: u64) -> BaseFeeParams;
    pub fn bypass_prevrandao(self, chain_id: u64) -> bool;
    pub fn inject_precompiles(
        self,
        precompiles: &mut PrecompilesMap,
        context: NetworkExecutionContext,
    ) -> Result<(), PrecompileCompositionError>;
    pub fn precompile_labels(
        self,
        tempo_hardfork: Option<TempoHardfork>,
    ) -> AddressHashMap<String>;
    pub fn precompile_inventory(
        self,
        tempo_hardfork: Option<TempoHardfork>,
    ) -> BTreeMap<String, Address>;
}
```

这些 projections 是 seam 的一部分，不是后续 B20 implementation detail。否则 Celo、Tempo、
Optimism 的既有 runtime behavior 仍会迫使 caller 在 resolution 后读取 `NetworkConfigs`，破坏
“resolve once”。`NetworkStatePlan` 只表达 backend/genesis ownership；实际 HashKey seed 仍属于
后续 runtime ticket，且 fork mode 必须跳过。

`NetworkVariant::HashKey` 只在 `hashkey` capability 编译时存在。它 resolve 为
`EvmFamily::Optimism`，设置 `hashkey = true`，并拥有版本化 standalone defaults：
`activation_time = 0`、
`activation_admin = 0xCB00000000000000000000000000000000000000`。plain Optimism
profile 使用相同 family，但 `hashkey = false`。

Cargo feature 必须从四个 binaries 及其 runtime crates 一直传播到
`foundry-evm-networks/hashkey`。v1.7.1 已无条件编译 Optimism EVM types，因此“HashKey
implies Optimism”由 `EvmFamily::Optimism` 表达；仅启用 build capability 不能激活 runtime
profile。

## 唯一 resolution contract

`NetworkConfigs::resolve(self) -> ResolvedNetworkProfile` 是 configuration 转换为 runtime
semantics 的唯一入口。command-boundary call chain 是：

```text
CLI/TOML -> NetworkConfigs
              |
              +-- command-local selector normalization
              +-- 该 command 自己负责的 optional fork identity inference
              v
       NetworkConfigs::resolve()       exactly once
              |
              v
       ResolvedNetworkProfile
              |
              +-- generic EVM family dispatch
              +-- EvmOpts environment and fork helpers
              +-- Backend / CreateFork / MultiFork
              +-- InspectorStack / ordinary / inspected / nested / isolated EVM
              +-- command runtime state and trace executor
```

`EvmOpts::env()`、`Backend::spawn()` 等 convenience method 可以保留 Ethereum-default
compatibility wrapper，但已进入 resolved command runtime 的路径必须调用显式 variants：

```rust
EvmOpts::env_with_network_profile(profile)
EvmOpts::fork_evm_env_with_network_profile(provider, profile)
EvmOpts::get_fork_with_network_profile(config, chain_id, block, profile)
Backend::spawn_with_network_profile(fork, profile, context)
```

这些 wrapper 不是合法的内部 transport edge；它们只服务尚未进入 resolved runtime 的 caller。

## v1.7.1 transport checklist

### Core execution 与 forks

| Path | 当前 v1.7.1 constructor | Required profile carrier | Acceptance condition |
| --- | --- | --- | --- |
| Environment | `EvmOpts::env` / `fork_evm_env` | 显式 `profile` 参数 | block normalization 使用传入 profile；不再从 `self.networks` 投影 runtime behavior。 |
| Initial fork | `EvmOpts::get_fork` | `CreateFork.network_profile` | fork request 携带 command-resolved value。 |
| Multi-fork create | `MultiFork::create_fork` -> async `create_fork` | `CreateFork.network_profile` | remote env construction 使用 request profile。 |
| Multi-fork roll | `MultiForkHandler::on_request(Request::RollFork)` | cloned `CreateFork` | roll fork 时 profile 原样保留。 |
| Backend root | `Backend::spawn` / `Backend::new` | `Backend.network_profile` + constructor context | resolved caller 使用 `spawn_with_network_profile(fork, profile, context)`；default 只用于 compatibility。 |
| Backend clones | `Backend::clone` / `clone_empty` | copied backend field；`clone_empty(context)` | clone 或 isolation preparation 不能重置 profile，新的 journal 使用当前 execution context。 |
| Fork-backed child | `Backend::new_with_fork` | 显式 `profile` 参数 | transaction replay/nested commit 收到 inspector-owned value。 |
| Database abstraction | `DatabaseExt` 与 `CowBackend` | `network_profile()` | cheatcodes 与 nested execution 从 backend 读取 value，不持有 config。 |
| Backend replay | `Backend::replay_until` | backend field | bare replay EVM 使用同一 profile 与 replay block timestamp 注入 precompiles。 |
| Warm precompile set | `BackendInner::precompile_addresses` -> `new_journaled_state` | 显式 `profile` + 当前 `NetworkExecutionContext` | bare EVM 构造出的 warm-address set 包含当前 activation snapshot 的 profile precompiles；root、`clone_empty`、new fork 均不得使用 default map。 |
| Inspector builder | `InspectorStackBuilder` | `network_profile` field/builder method | builder 不再存放 `NetworkConfigs`。 |
| Inspector runtime | `InspectorStack`、`InspectorStackInner`、`InspectorStackRefMut` | copied field + `InspectorExt::get_network_profile()` | ordinary 与 inspected construction 看到 equal value。 |
| Ordinary EVM | `EthEvmFactory`、`OpEvmFactory`、`TempoEvmFactory` | inspector accessor | injection 收到 `{ chain_id, creation timestamp }`。 |
| Nested EVM | `create_foundry_nested_evm` via `with_cloned_context` | existing inspector reference | nested call 内不 resolve、不 default。 |
| Isolation | `InspectorStackInner::with_nested_evm` 与 backend child creation | inspector/backend value | isolated call 保留 outer resolved value。 |
| Cheatcode fork | `create_fork_request` in `crates/cheatcodes/src/evm/fork.rs` | `ccx.ecx.db().network_profile()` | 新 `vm.createFork` 不重新 resolve cloned `EvmOpts`。 |

### Command surfaces

| Surface | Resolution point | Required downstream ownership |
| --- | --- | --- |
| Forge test/coverage | `TestArgs::run_tests` 中 `infer_network_from_fork()` 之后 | profile 传入 generic dispatch、env/fork helpers、`MultiContractRunnerBuilder`、`TestConfig`、backend、inspectors 和 debugger traces。v1.7.1 没有 mutation runner，因此排除 master-only mutation path。 |
| Script | `ScriptArgs::run_script` 中 `resolved_evm_opts()` 之后 | profile 传过 `preprocess`、`prepare_bundled`、`run_generic_script`、`ScriptConfig`、backend cache、executor、simulation 与 broadcast projections。 |
| Cast call | `CallArgs::run` 中 extract 一次 `EvmOpts`；若 `self.tx.tempo.is_tempo()`，先设置 `evm_opts.networks = NetworkConfigs::with_tempo()`，否则执行 fork inference，然后 resolve 一次 | `run_with_network` 必须同时接收 `EvmOpts` 与 profile，不能再 extract 一份 unresolved copy；profile 继续传到 `TracingExecutor`。 |
| Cast run | `RunArgs::run` 中 fork inference 之后 | `run_with_evm` 接收 resolved `EvmOpts` 与 profile；block replay 和 debugger 使用同一 value。 |
| Trace executor | caller-owned resolution | `TracingExecutor::get_fork_material` 与 `TracingExecutor::new` 接收 profile；只有 caller 需要延续相同 runtime 时才返回它。内部不调用 `with_chain_id()` 或 `resolve()`。 |
| Chisel | `run_command` 中 load/infer `EvmOpts` 之后 | HashKey dispatch 到 `OpEvmNetwork`；profile 存入 `SessionSourceConfig`，在 `!load`/session rebuild 时保留，并传给 backend/inspectors。 |
| Verify bytecode | `VerifyBytecodeArgs::run` 中 config/chain resolution 之后 | 按 profile family dispatch；profile 传过 tracing executor、genesis predeploy simulation、creation-block replay 与 `configure_env_block`，不能 hard-code `EthEvmNetwork`。 |
| Anvil | `NodeConfig::setup` 在 EVM/backend construction 前从最终显式 `NetworkConfigs` resolve 一次 | profile 传到 hardfork/base-fee projection、fork setup、`mem::Backend::with_genesis`、reset-fork、inventory、trace decoder、ordinary/inspected EVM 与 local state plan。fork mode 不得 seed profile-owned local state。 |

### Non-loss rules

- `NetworkConfigs` 可以继续存在于 serialized user config 与 `CheatsConfig`，但 resolution 后的
  runtime decision 只使用 `ResolvedNetworkProfile`。
- command-local selector 必须在 resolution 前归一化到 `NetworkConfigs`。Cast Tempo transaction
  args、Script fee token、Anvil explicit network/hardfork 等既有入口不能在 profile 之外形成第二条
  runtime decision path。
- generic dispatch 只检查 `profile.is_tempo()` / `profile.is_optimism()`。不存在
  command-local `is_hashkey()` branch；HashKey 与 Optimism 共用 `OpEvmNetwork`。
- EVM creation 按 creation timestamp 固定 B20 activation snapshot。后续 `vm.warp` 只影响下一次
  EVM construction，不回溯修改已建立的 precompile map。
- `Backend::replay_until` 是 bare EVM path，必须从 `Backend.network_profile` 注入；inspector
  无法补救该路径。
- `BackendInner::precompile_addresses()` 也是 bare EVM path。它必须改为接收 profile 与实际
  execution context；root backend 使用 caller 已构造的 `evm_env`，fork 使用 async
  `create_fork` 返回的 fork env。不能用 default timestamp/chain ID 生成 B20 warm-address set。
- `CreateFork` 是 initial fork、multi-fork、fork roll 与 cheatcode fork creation 的 canonical
  carrier。profile 不参与 `ForkId` cache identity；URL 与 block 继续承担该职责。
- Anvil 使用独立 backend type，但遵守相同 ownership：一个 profile field 同时服务 normal、
  inspected transaction path 与 reset-fork。

## Implementation audit boundary

后续 source implementation 应保持两个 commits：

1. **Seam commit**：定义 `ResolvedNetworkProfile`，替换 runtime `NetworkConfigs` projection，
   并通过 direct command construction paths 建立显式 carriers。
2. **Transport commit**：覆盖 fork/multi-fork/roll、backend replay、nested/isolation、script
   simulation、verify replay、Cast trace/debugger、Chisel session rebuild 与 Anvil reset-fork。

这会在不假设 master file layout 的前提下，重建 `775946811` -> `a56f34ac1` 的 review
boundary。

## 后续实现的 validation plan

transport tests 必须经 public seams 观察行为，并将独立选择的 literal profile 与 destination
观察到的 profile 比较：

- `foundry-evm-networks`：HashKey resolve 为 Optimism family + enabled B20 config；Ethereum、
  Optimism、Tempo、Celo 保持 v1.7.1 behavior。
- `InspectorStack`：builder、owned stack、mutable view、nested EVM、isolation 观察到 equal
  profile。
- fork core：create、roll、multi-fork reuse、cheatcode-created fork、backend clone、bare replay
  与 `new_journaled_state` warm-address construction 均保留 profile/context。
- command vertical slices：Forge、Script、Cast、Chisel、Verify、Anvil 将 profile 传到 destination
  constructor，且没有第二次 resolution。
- normal/inspected equivalence：两个 factory 收到相同 profile 与 creation timestamp。
- non-HashKey regression：既有 v1.7.1 focused suites 与最终 workspace suite 无变化，并分别
  证明 Celo labels/inventory、Tempo state plan/base fee、Optimism family dispatch 与 Ethereum
  defaults 没有改变。
