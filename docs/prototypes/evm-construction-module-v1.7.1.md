# v1.7.1 EVM construction deep module 规范

> 状态：architecture contract 已由 issues #45-#52 在 `v1.7.1-hsk-b20` 实现；opaque
> `ResolvedEvmOpts` handoff 由 command profile resolution seam 提供。
>
> 适用分支：`v1.7.1-hsk-b20`。
>
> 本文深化 `ResolvedNetworkProfile` transport seam。它取代
> [`resolved-network-profile-v1.7.1.md`](./resolved-network-profile-v1.7.1.md) 中由 caller 显式调用
> `*_with_network_profile` 并分别装配 backend、inspector 与 decoder 的方案；既有 resolution、
> EVM family dispatch、standalone genesis 与 fork no-seed contract 保持不变。
>
> 后续 Candidate 2 已将 command-owned resolution choreography 深化为
> [`command-profile-resolution-module-v1.7.1.md`](./command-profile-resolution-module-v1.7.1.md)。
> 因此本文 conceptual `prepare(evm_opts, config, resolved_profile)` handoff 应收紧为消费 opaque
> `ResolvedEvmOpts`；construction module 内部的 snapshot/backend/inspector/decoder ownership 不变。

## 1. 决策摘要

Foundry 应在每次 EVM creation point 建立一个 deep construction module。command/session 继续只
resolve 一次 immutable `ResolvedNetworkProfile`；construction module 根据该 profile 与当前
`EvmEnv` 唯一派生本次 execution 的 `NetworkExecutionContext` 和 B20 activation snapshot，并将
同一 snapshot 原子投影到 backend、inspector、normal/traced execution、nested/isolation execution
与 trace decoder。

module 的 conceptual interface 收敛为：

```rust
EvmConstruction::prepare(resolved_evm_opts, config)
    -> Result<PreparedEvm<FEN>>

PreparedEvm::construct(executor_config)
    -> Result<ConstructedEvm<FEN>>
```

`PreparedEvm` 与 `ConstructedEvm` 是 opaque return objects。caller 不接触 raw
`NetworkExecutionContext`，不分别设置 network profile/context，也不能通过 `into_parts()` 拆散
construction invariant。

合法 execution 的 CLI、JSON、trace、state transition 与 activation behavior 必须保持不变。
非法 profile/context pairing 应在 construction seam 返回 typed error，或在 interface 上不可构造；
不得继续依赖 `ExecutorBuilder::build` 的 runtime `assert_eq!`。

## 2. 问题与来源

`ResolvedNetworkProfile` 最初建立了正确的 command-owned resolution point，但 issue #30 的
`2a905d502`（`fix(evm): preserve resolved profile lifecycles`）把 transport responsibility 分配给
各层 caller：

- `EvmOpts` caller 选择 `env_with_network_profile` 与 `get_fork_with_network_profile`；
- caller 根据 `EvmEnv` 手工构造 `NetworkExecutionContext`；
- caller 把 profile/context 分别传入 `CreateFork`、`Backend`、`InspectorStack` 与 decoder；
- nested、isolation、replay 与 trace path 各自重新恢复同一组 invariant。

随后提交继续修补同一 locality 缺口：

| Commit | 扩散 surface |
| --- | --- |
| `b77fac4b9` | Forge |
| `e3152ad3a` | Script / Verify |
| `cc1db83f3` | Cast |
| `d7751ae3e` | Anvil |
| `a252fe60d` | Chisel |
| `e4f5d1f3f` | backend/inspector mismatch contraction |

当前 branch 中，相关显式 helper/setter 分布在 core、Forge、Script、Cast、Anvil、Chisel 与 traces。
典型 assembly path 包括：

| Consumer | 当前 assembly point |
| --- | --- |
| Forge | `crates/forge/src/cmd/test/mod.rs` |
| Script | `crates/script/src/lib.rs` |
| Chisel | `crates/chisel/src/executor.rs` |
| Tracing | `crates/evm/evm/src/executors/trace.rs` |
| Anvil | `crates/anvil/src/eth/backend/mem/mod.rs` |
| Cast | `crates/cast/src/debug.rs` |

`crates/evm/evm/src/executors/builder.rs` 只能在所有对象已经分别构造后，用 `assert_eq!` 拒绝
backend/inspector profile mismatch。这证明当前 interface 允许 caller 先构造非法状态，再依赖末端
检查补救；它没有隐藏 construction complexity。

## 3. 目标

1. **Leverage**：一个 interface 同时服务 Forge、Script、Verify、Cast、Anvil、Chisel、normal、
   traced、nested 与 isolated execution。
2. **Locality**：profile resolution 之后的 env/fork/backend/inspector/decoder choreography 只在一个
   module 内维护。
3. **一致性**：一次 EVM creation 只派生一个 activation snapshot，所有 projection 使用同一值。
4. **不可误用**：caller 不再能够分别选择 backend profile、inspector profile 或 decoder context。
5. **兼容性**：合法 execution 的 observable result 不变；非法 construction 更早、结构化失败。
6. **可测试性**：测试与 caller 穿过同一个 construction interface，不再测试各 transport helper。

## 4. 非目标

- 不改变 `NetworkConfigs::resolve()` 的 command/session ownership。
- 不新增 TOML、environment variable 或 CLI network setting。
- 不改变 HashKey B20 semantic baseline、binding pin、activation schedule 或 precompile implementation。
- 不把 standalone local genesis seed 移到每次 EVM creation；seed 仍只发生在 backend/genesis
  establishment，RPC fork 仍不得 seed 或覆盖远端状态。
- 不改变 fork cache identity、fork block pinning 或 state snapshot/revert semantics。
- 不改变 stdout/stderr contract、JSON schema、trace schema 或正常路径日志。
- 不建立通用 plugin framework；该 module 是 Foundry in-process EVM construction seam。

## 5. Ownership 与生命周期

construction module 必须区分三个生命周期，不能把它们合并成一个长生命周期 context object。

### 5.1 Command/session truth

`ResolvedNetworkProfile` 是一次 command/session 的 immutable truth：

- command-local selector 与 optional fork inference 在 `resolve()` 前完成；
- command entrypoint 只调用一次 `NetworkConfigs::resolve()`；
- generic EVM dispatch 只读取 `ResolvedNetworkProfile::evm_family()`；
- module 不重新 resolve，也不从 chain ID 猜回另一个 profile。

### 5.2 Reusable execution state

backend、fork database、journaled state 与 caller session cache 可以跨多次 EVM creation 复用。它们
承载 state lifecycle，但不拥有一个可无限复用的 B20 activation snapshot。

standalone local state plan 在 backend/genesis establishment 时执行一次。fork backend 保留远端状态，
不得因为后续 EVM creation 或 profile projection 重播 local seed。

### 5.3 Per-creation execution truth

每次 `PreparedEvm::construct(...)` 必须从当前 `EvmEnv` 派生：

```rust
NetworkExecutionContext::new(
    evm_env.cfg_env.chain_id,
    evm_env.block_env.timestamp().saturating_to(),
)
```

该 context 与其 B20 activation snapshot 在本次 constructed EVM 内固定：

- timestamp `< activation_time` 时不安装 B20 precompiles；
- timestamp `>= activation_time` 时安装 B20 precompiles；
- 同一 EVM 内 `vm.warp` 不追溯改变已经创建的 precompile map；
- 下一次 EVM creation 根据新的 `EvmEnv` 重新派生 snapshot；
- nested/isolation execution 继承 parent construction snapshot，不重新 resolve 或 default；
- fork roll、Anvil reset 或新的 replay environment 只有在创建新 EVM 时才产生新 snapshot。

## 6. Seam placement

external seam 应位于 `foundry-evm`，建议实现目录为：

```text
crates/evm/evm/src/construction/
```

`foundry-evm` 是同时可见 `EvmOpts`、`Backend`、`ExecutorBuilder`、`InspectorStack` 与 trace types 的
最低共同 crate。`foundry-evm-core`、`foundry-evm-traces` 与各 EVM factory 可以保留 private/internal
seams，但 caller-facing construction interface 不应下沉或复制到这些 crate。

```text
Command-owned resolution
        |
        | ResolvedNetworkProfile
        v
+---------------------------------------------+
| EVM construction module                     |
|                                             |
| prepare: env + fork pin + reusable state    |
| construct: per-creation snapshot            |
|            backend + inspector + decoder    |
|            nested/isolation inheritance     |
+---------------------------------------------+
        |
        v
ConstructedEvm<FEN>
```

Anvil 可以在 module 内使用 role-specific adapter 连接其独立 backend implementation，但该 adapter
是 internal seam，不能形成第二套 caller-facing profile/context assembly interface。

## 7. Interface contract

以下 Rust 只约束 interface shape，不提前固定 lifetime、generic parameter 与 error enum 的最终拼写：

```rust
let prepared = EvmConstruction::prepare::<FEN>(
    &evm_opts,
    &config,
    resolved_profile,
)
.await?;

let constructed = prepared.construct(ExecutorConfig {
    trace_mode,
    cheatcodes,
    coverage,
    gas_limit,
    isolation,
    // no network profile or execution context fields
})?;
```

### 7.1 `prepare`

`prepare` 隐藏并保证：

- 使用 caller 已经 resolve 的 profile；
- 获取或建立当前 `EvmEnv` 与 `TxEnv`；
- pin initial fork block，并创建内部 fork material；
- 建立或取得 module-owned reusable backend state；
- 保留 local/fork state-plan distinction；
- 不创建可由 caller 修改的 network context。

`PreparedEvm<FEN>` 可以向 caller 暴露只读 execution metadata，但不得暴露：

- mutable `ResolvedNetworkProfile`；
- raw `NetworkExecutionContext`；
- 可单独传给 backend/inspector/decoder 的 network fields；
- `into_parts()` 或等价的 tuple decomposition。

### 7.2 `construct`

`construct` 消费 `PreparedEvm<FEN>`，并保证：

- 从其当前 `EvmEnv` 派生唯一 snapshot；
- backend EVM projection、inspector 与 decoder 使用同一 snapshot；
- ordinary 与 traced construction 只改变 inspector behavior，不改变 network semantics；
- nested/isolation operation 由 `ConstructedEvm` 提供，并继承该 snapshot；
- reusable backend state 的 cache/rebinding policy 留在 module implementation 内；
- precompile composition failure 在返回 constructed value 前 fail closed。

`ExecutorConfig` 只表达 consumer-specific behavior，例如 trace mode、cheatcodes、coverage、gas、
legacy assertions、CREATE2 deployer 与 isolation。它不得出现 profile、network variant、chain-inferred
runtime semantics 或 execution context。

### 7.3 `ConstructedEvm`

`ConstructedEvm<FEN>` 是一次完整 execution construction 的 owned result。它可以提供：

- execution/runner 所需的 EVM handle；
- 已绑定同一 snapshot 的 trace decoder；
- nested/isolation operation；
- 只读 `EvmEnv`/chain/timestamp metadata。

它不得提供独立替换 network profile/context 的 setter，也不得暴露可与另一 construction result
重新配对的 raw backend、inspector 或 decoder parts。

## 8. Consumer migration contract

| Consumer | 迁移后 responsibility |
| --- | --- |
| Forge | resolve profile；提交 non-network runner config；消费 `ConstructedEvm`。不再组装 env/fork/backend/inspector profile。 |
| Script | resolve profile；提交 script inspector/config；backend cache 交由 module state policy。simulation 与 on-chain projection 不重新构造 network context。 |
| Verify | 复用 Script/Tracing construction adapter；genesis simulation 与 replay 不建立独立 path。 |
| Cast call/run/debug | resolve profile；提交 trace/debug config；decoder 从 constructed result 获取。 |
| Chisel | session 保留 immutable profile selector；每次 rebuild 通过 construction interface，不能直接复用带 stale context 的 backend。 |
| Anvil | setup 拥有 profile resolution；normal、inspected、reset-fork 与 per-transaction decoder 经同一 module-owned adapter。 |
| TracingExecutor | 退化为 constructed execution 的 role adapter，不再拥有独立 env/fork/backend/profile choreography。 |

任何 consumer-specific adapter 都只能配置 non-network behavior。若 adapter 需要直接设置 profile 或
context，说明 construction seam 被穿透，implementation 不满足本规范。

## 9. Compatibility 与 deletion contract

在 `v1.7.1-hsk-b20` 上采用 hard cutover。所有 workspace caller 必须迁移到 canonical interface，
不保留并行的 resolved-profile construction path。

应删除或降为 module-private implementation detail 的 branch-added interface 包括：

- `EvmOpts::env_with_network_profile`；
- `EvmOpts::fork_evm_env_with_network_profile`；
- `EvmOpts::get_fork_with_network_profile`；
- `Backend::spawn_with_network_profile`；
- `Backend::new_with_network_profile`；
- inspector 独立 `network_profile(...)` setter；
- `CallTraceDecoderBuilder::with_network_profile(profile, context)`；
- 其他允许 caller 分别传递 profile/context 的 constructor 或 setter。

v1.7.1 baseline 已有 convenience constructor 只在以下条件下保留：

1. 它能完整委托 canonical construction interface；或
2. 它明确限制为不需要 profile-owned execution semantics 的 compatibility path，并对 HashKey 或
   其他无法安全派生 context 的 profile fail closed。

不得以 `NetworkExecutionContext::default()` 静默填补缺失信息。不得保留 deprecated branch-only
wrapper：这些 interface 尚未构成 v1.7.1 已发布 contract，过渡期只会永久保留第二条 assembly path。

## 10. Observable behavior contract

对所有合法 construction：

- stdout/stderr channel contract 不变；
- `--json` schema 与字段不变；
- exit code 不变；
- trace labels、decoded calldata、returns、events 与 custom errors 不变；
- B20 precompile availability、gas、logs 与 state transition 不变；
- activation timestamp `99/100/101` edge behavior 不变；
- normal/traced execution semantic equivalence 不变；
- 正常路径不新增 banner、warning 或 status prose。

唯一允许的行为变化是非法 construction：

- profile family 与 selected generic EVM 不兼容；
- fork/backend/profile source 不一致；
- precompile composition 失败；
- compatibility constructor 缺少派生 snapshot 所需的 `EvmEnv`。

这些情况必须在 construction seam 返回 typed error，错误进入 stderr；不得 panic，也不得延迟到
transaction execution 后才失败。

## 11. Test contract

测试必须穿过 deep module interface，遵循 replace-don't-layer：新的 interface tests 建立后，删除只
验证旧 transport helper 或 internal setter 的测试。

### 11.1 Construction invariant

- normal 与 traced construction 观察到相同 profile/snapshot behavior；
- backend EVM projection、inspector、decoder、nested/isolation 使用同一 snapshot；
- caller config 不存在 network setter；
- precompile composition failure 在 `construct` 返回前失败。

测试断言 observable outcome，例如 precompile availability、trace identity 与 state diff；不读取
private snapshot fields。

### 11.2 Activation lifecycle

- deterministic fixture 在 timestamp `99` 不启用；
- timestamp `100` inclusive activation；
- timestamp `101` 持续启用；
- 同一 constructed EVM 中 `vm.warp` 不改变既有 precompile map；
- 新 EVM construction 从新的 environment 派生新 snapshot。

### 11.3 Fork、state 与 isolation

- standalone local genesis seed 只执行一次；
- fork creation、roll、replay 与 Anvil reset 不 seed 或覆盖远端 B20 state；
- reusable backend state 不携带 stale activation snapshot；
- nested/isolation execution 继承 parent snapshot；
- snapshot/revert 继续只使用现有 journal/database mechanism。

### 11.4 Consumer integration

保留或迁移 Forge、Script、Verify、Cast、Anvil、Chisel 的 HashKey vertical slices。合法路径的 CLI
snapshot 原则上不更新；若 snapshot 变化，必须先证明旧输出属于非法 construction diagnostic。

### 11.5 Non-HashKey regression

覆盖 Ethereum、Optimism、Tempo 与 Celo 的 family dispatch、base fee、precompile、trace 与默认
behavior，证明 deep module 没有把 HashKey semantics 变成 build-time 或 global activation。

### 11.6 替换旧 mismatch test

删除 `ExecutorBuilder` 中依赖 `#[should_panic]` 的
`rejects_a_silently_defaulted_inspector_profile`。用 construction-interface coherence test 替换它：
合法 caller 只能获得完整 `ConstructedEvm`，不能分别构造 mismatch。

workspace-wide build/check 负责证明旧 branch-only interface 已从全部 caller 删除；不为此单独引入新的
compile-fail test framework。

## 12. Migration order

source implementation 应按 dependency direction 迁移，避免在中间 commit 同时保留两套合法 path：

1. 在 `foundry-evm` 建立 opaque construction interface 与 focused invariant tests。
2. 把 core backend、inspector、factory 与 trace decoder 的 network-bearing constructor 降为内部
   implementation detail。
3. 迁移 Forge、Script、Verify 与 `TracingExecutor`。
4. 迁移 Cast、Chisel 与 Anvil role adapters。
5. 删除 branch-added `_with_network_profile` interface 与末端 `assert_eq!` panic test。
6. 运行 consumer vertical slices、activation/fork lifecycle 与 non-HashKey regression。

每个中间 slice 必须保证已迁移 caller 只穿过 canonical interface。若为了编译临时保留 wrapper，
wrapper 必须 module-private，并在同一 migration series 内删除。

## 13. Acceptance criteria

- [ ] command/session 仍只 resolve 一次 `ResolvedNetworkProfile`。
- [ ] 每次 EVM creation 只派生一个 `NetworkExecutionContext`/activation snapshot。
- [ ] caller interface 不包含 profile/context pairing responsibility。
- [ ] Forge、Script、Verify、Cast、Anvil、Chisel 与 tracing path 复用同一 construction module。
- [ ] nested/isolation 继承 parent snapshot。
- [ ] backend/fork cache 不保存可跨 creation 误用的 stale activation snapshot。
- [ ] standalone local seed 与 fork no-seed contract 不变。
- [ ] branch-added parallel constructors/setters 已删除或降为 module-private。
- [ ] `ExecutorBuilder::build` 不再用 panic 修补 profile mismatch。
- [ ] 合法 CLI/JSON/trace/state outputs 不变。
- [ ] interface tests 替换旧 transport-helper tests。
- [ ] activation、fork/isolation、consumer 与 non-HashKey regressions 通过。

## 14. Rejected alternatives

### Session-scoped profile/context object

把 `ResolvedNetworkProfile` 与 `NetworkExecutionContext` 固化为一个长生命周期对象，会在 fork roll、
Anvil reset、replay environment 或下一次 EVM creation 后产生 stale timestamp。它也会错误地暗示
`vm.warp` 应修改当前 EVM snapshot。

### 保留显式 transport helpers

`*_with_network_profile` 只是把 implementation 顺序暴露给 caller。删除这些 helper 会让相同
参数与顺序重新散回 Forge、Script、Chisel 与 tracing，说明它们没有形成 deep module。

### 末端 equality assertion

`ExecutorBuilder::build` 的 `assert_eq!` 只能发现已经构造出的非法 pairing，不能让非法状态不可
表示，也不能覆盖 decoder、bare replay 或 Anvil 的平行 path。

### 返回 raw parts

返回 `(env, fork, backend, inspector, decoder, profile, context)` tuple 或提供 `into_parts()`，会把
construction invariant 重新交还 caller，module 退化为 pass-through wrapper。

## 15. Deletion test

删除当前 `_with_network_profile` methods，只会把 profile/context 参数和调用顺序搬回各 consumer；
复杂度没有消失。

删除本规范定义的 deep construction module，则 env/fork preparation、per-creation snapshot、backend
state policy、inspector/decoder binding、nested/isolation inheritance 与 typed failure 必须同时重新
散布到 Forge、Script、Verify、Cast、Anvil、Chisel 与 tracing。该复杂度回流证明 module 具有足够
depth、leverage 与 locality。
