# v1.7.1 command profile resolution deep module 规范

> 状态：已确认 architecture contract，source implementation 已落地于
> `crates/evm/core/src/opts/resolution.rs`；后续 command consumer migration 仍按 issues #55-#63 推进。
>
> 适用分支：`v1.7.1-hsk-h20`。
>
> 本文深化 architecture review Candidate 2。它取代 Forge、Script、Verify、Cast、Chisel 与
> Anvil 各自维护的 selector precedence、chain/fork inference 和 exactly-once resolution
> choreography。后续 EVM assembly 仍由
> [`evm-construction-module-v1.7.1.md`](./evm-construction-module-v1.7.1.md) 所有；
> `NetworkConfigs::resolve()` 继续是 configuration 到 immutable runtime semantics 的纯转换。

## 1. 决策摘要

Foundry 应在 `foundry-evm-core::opts::resolution` 建立 command profile resolution deep module。
command 只负责把 fee token、Tempo transaction、configured chain、hardfork 或 Anvil execution
override 等 command-local 语义归一化为小型 `NetworkIntent`；module 负责：

1. 合并显式 selector 与 command requirement；
2. 拒绝不兼容的 requirement，而不是执行 silent override；
3. 在尚未选择 profile 时按 command chain hint、fork identity、Ethereum default 顺序推断；
4. 只调用一次 `NetworkConfigs::resolve()`；
5. 返回 opaque resolved carrier，使 options/profile pairing 不能被 caller 拆散或重新组合；
6. 把同一个 carrier 交给 EVM construction module。

conceptual interface 是：

```rust
CommandProfileResolution::resolve_evm_opts(evm_opts, network_intent)
    -> Result<ResolvedEvmOpts, NetworkResolutionError>

ResolvedEvmOpts::network_profile(&self)
    -> ResolvedNetworkProfile

EvmConstruction::prepare(resolved_evm_opts, config)
    -> Result<PreparedEvm<FEN>, EvmConstructionError>
```

`ResolvedEvmOpts` 是 opaque return object。它可以提供只读 options 与 profile projection，但不得
提供 mutable `NetworkConfigs`、独立 profile setter 或 `into_parts()`。command 必须在 resolution
前完成自己的 options normalization；resolution 后只能配置与 network identity 无关的 consumer
behavior。

合法 invocation 的 stdout、stderr、JSON schema、exit code 与 execution semantics 必须保持不变。
过去被 silent override 或 silent Ethereum fallback 掩盖的非法/不确定 resolution，应在 EVM
construction 前以 typed error fail closed。

## 2. 当前问题与 source evidence

当前 branch 已完成 immutable `ResolvedNetworkProfile` transport 和 opaque EVM construction，但
profile 选择 choreography 仍散布在 command caller：

| Consumer | 当前 choreography |
| --- | --- |
| Forge | `infer_network_from_fork().await` 后调用 `evm_opts.networks.resolve()`。 |
| Script | `fee_token` 直接覆盖 `NetworkConfigs`，否则调用 fork inference，随后 caller 再 resolve。 |
| Cast call | Tempo transaction、`--chain`、fork inference 与 resolve 由 command 手工排序。 |
| Cast run | command 自己执行 fork inference 与 resolve。 |
| Chisel | configured chain mutation、fork inference、config 回写与 resolve 连续发生。 |
| Verify | `resolve_verify_network_profile()` 维护一套 chain-to-network fallback。 |
| Anvil | `NodeConfig::setup()` 在 fork identity 可用前 resolve；`set_chain_id()` 还尝试承担 inference。 |

`crates/anvil/src/config.rs:659` 当前调用消费式
`self.networks.with_chain_id(chain_id)` 却丢弃返回值。现有 `set_chain_id` coverage 只观察数值 chain
ID 与 wallet 更新，没有证明 network profile inference。这不是一个孤立 typo：interface 同时存在
消费式 `NetworkConfigs::with_chain_id(self)`、变更式 `EvmOpts::infer_network_from_fork(&mut self)`
与末端 `resolve()`，允许 caller 漏掉返回值、改变顺序或多次 resolve。

另一个 source-level 缺口是显式 Ethereum 与 unresolved default 没有被 inference choreography
一致区分。显式 `--network ethereum` 应固定 Ethereum 并跳过 profile inference；default
`NetworkConfigs` 则仍可由 command/fork identity 推断。当前 guard 只检查
`is_tempo()`/`is_optimism()`，会为显式 Ethereum 进行无意义 RPC lookup。

`crates/evm/core/src/opts.rs` 还包含访问公网 `rpc.moderato.tempo.xyz` 的 flaky inference test。
command resolution 的 contract 不应依赖 live external RPC。

## 3. Module ownership 与 seam

### 3.1 External seam

external seam 位于 command-local input normalization 之后、fork identity lookup 与
`NetworkConfigs::resolve()` 之前：

```text
CLI / TOML / command-local args
                |
                | command-owned normalization
                v
      EvmOpts + NetworkIntent
                |
                v
   CommandProfileResolution
      - constraints
      - precedence
      - fork identity
      - exactly-once resolve
                |
                v
       opaque ResolvedEvmOpts
                |
                v
        EvmConstruction
```

module 不接受 `ScriptArgs`、`CallArgs`、`NodeConfig` 等 raw command types。否则 implementation
会依赖所有 CLI domain，interface 随每个 command option 增长，形成 shallow switchboard。

module 也不要求 caller 先查询 RPC 再提交 `ForkIdentity`。fork lookup 是否需要执行、何时跳过、
失败是否可以 fallback，都是 resolution interface knowledge，必须由 module 所有。

### 3.2 Dependency classification

selector、requirement、chain mapping 与 profile resolution 是 in-process dependency。fork RPC 是
true external dependency，因此在 module implementation 内建立 `ForkIdentitySource` port：

- production JSON-RPC adapter；
- deterministic in-memory adapter。

这是一个真实 internal seam：production 与 test adapter 都存在，但 port 不暴露给 command caller。
module 的 external interface 仍只有 command resolution。

production adapter 至少解析：

- `eth_chainId`；
- Anvil/Hardhat chain ID `31337` 下可选的 `anvil_nodeInfo.network` marker。

optional `anvil_nodeInfo` enrichment 不支持、方法不存在或没有 network marker，不属于 identity
transport failure。`eth_chainId` 在 resolution 必需时失败，才返回 typed error。

### 3.3 Anvil role adapter

EvmOpts-based consumer 使用 `ResolvedEvmOpts`。Anvil 使用 `NodeConfig`，不得为了复用 carrier 而把
完整 Node configuration 伪装成 `EvmOpts`。Anvil 可以拥有 private `ResolvedNodeConfig` role adapter，
但它必须消费同一个 `CommandProfileResolution` implementation，并遵守以下约束：

- 不复制 selector precedence；
- 不复制 chain/fork inference；
- 不公开第二个 resolution interface；
- standalone chain/genesis ID 可以归一化为 identity hint；
- fork `--chain-id` 只作为 execution override，不能覆盖 remote fork identity；
- optional fork identity snapshot 可由 adapter 私有保存并在 setup 中复用，不能暴露给 caller。

## 4. Network intent contract

`NetworkIntent` 只表达跨 command 可共享的事实，不携带 raw CLI types。conceptually 包含：

- optional exact profile requirement，例如 Tempo fee token 或 Tempo transaction；
- optional family constraint，例如 Optimism hardfork；
- optional identity-bearing chain hint；
- optional fork identity request；
- requirement source，用于 typed diagnostic。

requirement 与 hint 必须区分：

- requirement 是 execution 合法性的约束，不能被其他 selector silent override；
- hint 只在 profile 尚未确定时参与推断；
- execution chain override 不是 network identity hint。

hardfork constraint 按 family 兼容，而不是总按 exact profile 比较。例如 HashKey profile 属于
Optimism family，因此 Optimism hardfork 保留 HashKey；它不能把 HashKey 降级为 plain Optimism。

## 5. Precedence 与 conflict contract

resolution 采用“约束优先、推断降级”：

| 顺序 | Evidence | Contract |
| --- | --- | --- |
| 1 | 显式 `NetworkConfigs` selector | 固定 exact profile，包括显式 Ethereum。 |
| 2 | command requirement / hardfork constraint | 与已选 profile 合并；不兼容则 fail closed，不覆盖。 |
| 3 | command 声明的 identity-bearing chain hint | 仅在尚未选择时推断已知 profile。 |
| 4 | fork identity | 仅在尚未选择时查询并推断。 |
| 5 | 无可用 selection | resolve 为 Ethereum default。 |

### 5.1 Explicit selector

CLI/TOML/provider 已产生的 explicit selector 具有最高 identity authority。它包括：

- explicit Ethereum；
- Celo；
- Optimism；
- Tempo；
- HashKey。

显式 selector 存在时跳过 fork identity lookup。显式 profile 与 remote chain identity 可以有意不同；
module 不以 remote evidence 覆盖用户选择。

### 5.2 Requirements

command requirement 不“赢过”显式 selector，而是与之做一致性检查：

- Script `--tempo.fee-token` 要求 exact Tempo；
- Cast Tempo transaction 要求 exact Tempo；
- Tempo hardfork 要求 Tempo family；
- Optimism hardfork 要求 Optimism family，并与 HashKey compatible；
- Ethereum hardfork 要求 Ethereum family，并保留 compatible Celo semantics。

多个 requirement 不兼容时也返回 conflict，不使用 last-write-wins。

### 5.3 Chain hints

chain mapping 应返回“已知 inference”或 `None`，不能把所有未知 chain ID 立即解释为 explicit
Ethereum。否则 unknown `31337` 会在 `anvil_nodeInfo` enrichment 前过早锁定 Ethereum。

command-local normalization 决定一个 chain ID 是否 identity-bearing：

| Command fact | Intent |
| --- | --- |
| Cast `--chain` | identity hint。 |
| Chisel configured chain | identity hint。 |
| Verify resolved chain | identity hint。 |
| Anvil standalone chain/genesis ID | identity hint。 |
| Anvil fork `--chain-id` | execution override，不是 hint。 |

已知 command chain hint 比 fork identity 优先；unknown hint 允许继续查询 fork identity。

### 5.4 Fork identity 与 fallback

只有 selection 尚未完成且存在 fork endpoint 时，module 才访问 `ForkIdentitySource`：

- `eth_chainId` transport failure：typed error；
- known Tempo/Optimism/Celo chain：选择对应 profile；
- `31337` 且 node marker 为 Tempo：选择 Tempo；
- unknown chain：没有额外 selection，最终使用 Ethereum；
- optional node-info enrichment 不支持：不是错误。

不允许在需要 identity lookup 时吞掉 `eth_chainId` error 并按 Ethereum 执行。

## 6. Result interface

`ResolvedEvmOpts` 至少隐藏：

- normalized `EvmOpts`；
- immutable `ResolvedNetworkProfile`；
- optional cached fork identity implementation state。

它可以提供：

- `network_profile()` read-only projection，供 EVM family dispatch；
- read-only non-network option projection；
- 由 `EvmConstruction`、session adapter 或 command runtime 消费的 owned handoff。

它不得提供：

- mutable `NetworkConfigs`；
- `set_network_profile`；
- raw profile/options tuple decomposition；
- 允许 caller 用另一个 profile 重新配对 options 的 constructor；
- resolution 后再次调用 `with_chain_id()`、fork inference 或 `resolve()` 的接口。

command 在 resolution 前完成 sender、fee-token intent、configured chain 等 normalization。若 consumer
需要在 resolution 后配置 tracing、coverage、isolation 或 assertions，这些值应进入 Candidate 1 的
consumer config，而不是重新开放 mutable `EvmOpts.networks`。

## 7. Error contract

external error interface 保持很小：

```rust
enum NetworkResolutionError {
    ConflictingRequirement {
        configured: &'static str,
        required: &'static str,
        source: NetworkRequirementSource,
    },
    ForkIdentityUnavailable {
        source: ForkIdentityError,
    },
}
```

exact type spelling 可以随 implementation 调整，但 error taxonomy 不扩大为 provider-specific error
collection。`NetworkRequirementSource` 使用稳定的用户术语，例如：

- `--tempo.fee-token`；
- Tempo transaction；
- `hardfork`。

diagnostic example：

```text
Error: network requirement `tempo` from `--tempo.fee-token`
conflicts with configured network `hashkey`
```

```text
Error: failed to resolve network profile from fork identity:
`eth_chainId` request failed
```

error display 不得包含带 credentials/query token 的完整 RPC URL、request headers 或未经脱敏的
provider debug representation。

## 8. Consumer mapping contract

| Consumer | Command-owned normalization | Resolver-owned behavior |
| --- | --- | --- |
| Forge test/coverage | 普通 fork 生成 inherit intent；configured network 来自 loaded `EvmOpts`。 | fork identity、precedence、resolve 与 opaque carrier。 |
| Script | fee token 生成 exact Tempo requirement；其他 options 在 resolution 前完成。 | requirement conflict、fork identity 与 resolve。 |
| Cast call | Tempo transaction 生成 exact requirement；`--chain` 生成 identity hint。 | precedence、fork lookup 与 resolve。 |
| Cast run | 生成 inherit intent。 | fork identity 与 resolve。 |
| Chisel | configured chain 生成 identity hint。 | resolve 后 carrier 进入 session；reload 不重新推断。 |
| Verify | provider/config resolved chain 生成 identity hint。 | explicit profile preservation 与 resolve。 |
| Anvil | standalone chain/genesis ID 或 fork execution override 的 domain 区分。 | shared precedence、fork identity 与 profile resolution。 |

EVM family dispatch 继续由各 command 当前的 `ResolvedNetworkProfile::evm_family()` projection 驱动。
是否进一步深化重复 dispatch 属于 architecture review Candidate 5，不纳入本文。

## 9. Compatibility 与 deletion contract

在 `v1.7.1-hsk-h20` 上采用 hard cutover。所有 workspace command caller 必须迁移到 canonical
resolution interface，不保留能暴露中间 mutation 的 deprecated shim。

应删除：

- `EvmOpts::infer_network_from_fork()`；
- `NetworkConfigs::with_chain_id()`；
- Script `resolved_evm_opts()`；
- Verify `resolve_verify_network_profile()`；
- command-local selector mutation + inference + resolve choreography；
- production command 中裸 `evm_opts.networks.resolve()`；
- `EvmConstruction::prepare(evm_opts, config, profile)` 形式的松散 pairing；
- 只验证旧 mutation helper 内部状态的 tests。

应保留：

- `NetworkConfigs::resolve()`，作为纯 configuration transformation；
- `NetworkConfigs` constructors、serde 与 `normalize_for_hardfork()`；
- config serialization/display 所需的 `resolved_network()`、`active_network_name()` 等 query；
- `NodeConfig::with_chain_id()` / `set_chain_id()` 的 execution chain 与 wallet update 语义，但删除其
  network inference side effect；
- tests、static explicit-variant tooling 与 `foundry-evm-networks` implementation 内的直接
  `NetworkConfigs::resolve()`。

`EvmOpts::env()`、`fork_evm_env()` 只能在完整委托 canonical resolver、使用 inherit intent、遵守
fail-closed 并且不暴露中间 mutation 时作为 terminal compatibility adapter 保留。同步
`EvmOpts::get_fork()` 无法独立完成 async identity contract，应降为 module-private 并要求 resolved
carrier。

migration 期间若跨 crate 编译必须暂存旧签名，可以使用 `#[doc(hidden)]` workspace bridge。它不是
发布 compatibility contract，不得新增 caller，并必须在同一 migration series 的 hard-cut slice
删除。

final source audit 应禁止 production command path 出现：

```text
infer_network_from_fork
NetworkConfigs::with_chain_id
evm_opts.networks.resolve()
```

## 10. CLI output contract

合法 resolution 的 observable output 完全不变：

- stdout primary result 不变；
- stderr 不新增 status、warning 或 banner；
- `--json` success schema 不变；
- `--quiet` 与 verbosity 行为不变；
- exit code 仍为 `0`；
- EVM family、trace、state transition 与 H20/Tempo behavior 不变。

resolution failure 使用既有 CLI error projection：

| Failure | stdout | stderr | Exit code |
| --- | --- | --- | --- |
| requirement conflict | empty | `Error: ...` | `1` |
| required fork identity failure | empty | `Error: ...` | `1` |
| Clap argument conflict | empty | existing Clap diagnostic | `2` |

`--json` 不新增 JSON error schema；failure 仍写 stderr，stdout 为空。`--quiet` 不隐藏 error，verbosity
不改变 error text。hard cutover 不产生 deprecation warning。unknown chain fallback Ethereum、显式
selector 导致跳过 inference 都不产生 warning；debug provenance 只通过 `RUST_LOG` tracing 暴露。

## 11. Test contract

测试遵循 replace-don't-layer：完整 precedence matrix 只穿过
`CommandProfileResolution` external interface，各 command 只测试自己的 intent mapping。

### 11.1 Resolver interface matrix

至少覆盖：

- explicit HashKey + Tempo requirement -> typed conflict；
- explicit HashKey + Optimism family constraint -> HashKey；
- explicit Ethereum + Tempo fork -> Ethereum，identity adapter 零调用；
- no selector + known Tempo/Optimism/Celo chain hint -> corresponding profile；
- unknown chain hint + Tempo fork identity -> Tempo；
- `31337` + Tempo node marker -> Tempo；
- unknown fork identity -> Ethereum；
- required `eth_chainId` failure -> `ForkIdentityUnavailable`；
- explicit selector、requirement 或 known chain hint 完成 selection 后不访问 RPC。

tests 只能通过 opaque result 的 read-only interface 观察 profile，不读取 private options/profile
pairing。

### 11.2 RPC adapter tests

production adapter 使用 local RPC stub 或 local Anvil，覆盖：

- plain Anvil -> Ethereum；
- Tempo Anvil -> Tempo；
- unsupported `anvil_nodeInfo` -> non-error；
- `eth_chainId` failure -> redacted typed error。

删除 `flaky_infer_network_tempo_moderato_rpc`；任何 test 不得依赖 live external RPC。

### 11.3 Consumer mapping tests

每个 consumer 只验证 domain-to-intent mapping：

- Forge inherit；
- Script Tempo fee-token requirement；
- Cast Tempo transaction requirement 与 chain hint；
- Cast run inherit；
- Chisel configured-chain hint/session retention；
- Verify resolved-chain hint/explicit HashKey preservation；
- Anvil standalone hint 与 fork execution override distinction。

不在每个 command 复制完整 precedence matrix。

### 11.4 CLI projection 与 construction handoff

Forge、Cast、Chisel、Anvil binary 各覆盖一个 resolution failure，使用 snapbox exact stderr、empty
stdout 与 failure status。一个 focused handoff test 证明 `ResolvedEvmOpts` 能直接由
`EvmConstruction` 消费；不重复 Candidate 1 已覆盖的 precompile、snapshot、decoder 与 state
lifecycle tests。

## 12. Dependency-ordered migration

source implementation 按依赖方向迁移：

1. **R1 Core contract**：在 `foundry-evm-core` 增加 intent、constraints、typed errors、internal port
   与 resolver matrix；不迁移 caller。
2. **R2 RPC adapter + opaque carrier**：增加 production/in-memory adapters 与
   `ResolvedEvmOpts`，删除 live-RPC flaky test。
3. **R3 Construction handoff**：让 `EvmConstruction::prepare` 消费 resolved carrier；不改变
   Candidate 1 内部 snapshot/backend/inspector/decoder ownership。
4. **R4 Forge**：迁移 Forge test/coverage resolution。
5. **R5 Script/Verify**：迁移 Script、Verify 与共享 tracing role。
6. **R6 Cast**：迁移 Call/Run intent mapping 与 resolution。
7. **R7 Chisel**：迁移 command 与 session resolution。
8. **R8 Anvil**：增加 private resolved role adapter，分离 standalone hint 与 fork override。
9. **R9 Hard cut**：删除旧 helpers、松散 handoff、mutation tests 与 migration bridge，运行 source
   audit 和 CLI failure snapshots。
10. **R10 Regression gates**：运行 resolver、construction、consumer vertical slices、HashKey/Tempo
    lifecycle 与 non-HashKey workspace checks。

R4-R8 在 R3 后可以独立迁移；R9 阻塞于全部 consumer migration。每个中间 slice 必须保持已迁移
caller 只使用 canonical interface。migration bridge 不得跨发布保留。

## 13. Acceptance criteria

- [ ] selector/requirement/chain/fork precedence 只在一个 implementation 中维护。
- [ ] explicit Ethereum 与 unresolved default 可区分。
- [ ] incompatible requirement 在 EVM construction 前返回 typed conflict。
- [ ] required fork identity failure 不静默 fallback Ethereum。
- [ ] unknown chain/fork identity 可以确定性 default Ethereum。
- [ ] explicit selection 或 sufficient local evidence 跳过 resolution RPC。
- [ ] command/session 只产生一个 immutable resolved carrier。
- [ ] Candidate 1 construction 直接消费该 carrier，不接受松散 profile pairing。
- [ ] Anvil 不复制 precedence/inference，fork `--chain-id` 不覆盖 remote identity。
- [ ] production command path 不再调用旧 mutation choreography。
- [ ] legal CLI/stdout/stderr/JSON/exit behavior 不变。
- [ ] resolution errors 使用 stderr、exit `1`，且不泄漏 RPC secrets。
- [ ] resolver matrix 替换旧 helper tests，不使用 live external RPC。
- [ ] family dispatch、H20 genesis、trace decoding 与 release gate ownership 不扩张。

## 14. Rejected alternatives

### 14.1 把 resolution 扩进 `foundry-evm-networks`

拒绝。`foundry-evm-networks` 当前是 pure semantics module。让它依赖 `EvmOpts`、provider 或 RPC 会
把 true external dependency 污染进 configuration projection，并反转 crate dependency direction。

### 14.2 Resolver 接受 raw command args

拒绝。它会依赖 Forge/Script/Cast/Chisel/Verify/Anvil 的 option types，使 interface 随 command
surface 增长，成为 shallow switchboard。

### 14.3 Caller 先查 fork identity，再调用 pure resolver

拒绝。caller 仍必须记住“何时查、何时跳过、失败能否 fallback”的 choreography，locality 缺口没有
消失。

### 14.4 返回 `(EvmOpts, ResolvedNetworkProfile)`

拒绝。tuple 允许 caller 拆散 pairing、替换 profile 或再次 mutate `EvmOpts.networks`，无法表达
exactly-once invariant。

### 14.5 保留 deprecated mutation helpers

拒绝。`with_chain_id -> infer_network_from_fork -> resolve` 会继续成为第二套 interface；deprecated
shim 只会延长并行 choreography 的寿命。

### 14.6 把 family dispatch 一并集中

拒绝。resolution 只产出 immutable profile；generic Rust family dispatch 是否深化属于 Candidate 5，
不应扩大 Candidate 2 scope。
