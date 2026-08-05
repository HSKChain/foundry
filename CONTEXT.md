# Foundry Network Context

Foundry 对不同 EVM 网络语义及其附加执行能力的统一领域语言。

## Language

**Network profile**:
用户选择或由链身份解析出的命名网络行为集合。
_Avoid_: Network mode, network variant（除非特指 Rust 类型 `NetworkVariant`）

**EVM family**:
一个 network profile 所基于的基础交易与执行语义家族，例如 Ethereum 或 Optimism。
_Avoid_: Base network, parent chain

**Network extension**:
叠加在 EVM family 之上的网络专属执行语义；它不改变该 profile 所属的 EVM family。
_Avoid_: Custom mode, special feature

**HashKey B20 profile**:
以 Optimism 为 EVM family，并附加 HSK B20 network extension 的 network profile。
_Avoid_: Plain Optimism, Ethereum with B20

**HashKey build capability**:
在 release archive 内的标准 `forge`、`cast`、`anvil`、`chisel` 二进制中编译 HashKey profile 支持的内部 Cargo feature `hashkey`；`hsk-foundryup` 只在安装层生成 `hsk-forge`、`hsk-cast`、`hsk-anvil`、`hsk-chisel` wrapper，不修改 Cargo `[[bin]]` 或 archive member 名。该 feature 依赖 `optimism` 并隔离 B20 dependency graph。HSKChain 官方构建默认启用它，但 B20 runtime 仍只由显式 `--network hashkey` / `network = "hashkey"` 激活；未编译该 feature 的构建不暴露 HashKey profile。
_Avoid_: Cargo-level renamed binaries, runtime activation by build alone

**HashKey profile selector**:
首版唯一用户配置 surface：具体命令的 `--network hashkey`，或 `foundry.toml` 的 `network = "hashkey"`；CLI 覆盖配置文件。B20 activation time/admin、feature seed、singleton marker、binding revision 均由版本化 profile 拥有，不提供独立 TOML、环境变量或 CLI override。
_Avoid_: User-assembled B20 config, selectable binding revision

**HashKey release identity**:
包含标准四个 archive binary、`hashkey` build capability 与独立 `hsk-foundryup` installer 的 HSKChain release tag，例如 `v1.7.2-hsk.1`。Release artifact 与 `--locked` source build 使用同一 `Cargo.lock`，并记录 Foundry、B20 semantic/binding、Tempo compatibility、Reth compatibility 的精确 revisions；installer 从 `HSKChain/foundry` 下载 archive，并以 `hsk-*` wrapper 与 stock Foundry 并存。
_Avoid_: Unversioned custom binary, moving dependency builds

**HashKey documentation contract**:
随 profile 实现共同交付的 README quick start、Foundry Book config reference、B20 local simulation guide 与 release revision mapping。文档必须覆盖 `hsk-*` namespaced commands、development admin/feature defaults、Forge prank 与 Anvil impersonation、marker/snapshot/cheatcode 边界，并明确 local defaults 非生产参数、RPC fork 不 seed；自动生成的 CLI reference 不手工修改。
_Avoid_: Usage-only documentation, unpublished production parameters

**HashKey supported delivery scope**:
首版正式兼容性声明只覆盖 Solidity caller compilation、standalone Forge/Anvil/Chisel execution，以及 Cast 连接由 `hsk-anvil --network hashkey` 启动的本地节点。RPC fork、远端 call/replay 与 broadcast 不声明 HashKey production/history fidelity，但仍必须 no-seed；profile 只承诺 pinned Beryl semantic baseline，不承诺 Cobalt 或未来 revision。
_Avoid_: Production chain support, historical fork fidelity

**HashKey release evidence**:
由 `.github/scripts/hashkey_release_gate.py` 拥有的两个不可裁剪 phase projection：`source` 在 release binaries 产生前执行完整 source membership；`artifact` 对每个 supported target 的最终 tar/zip archive 执行 native binary-surface 与 target-owned runtime evidence。Source phase 不等于 release eligibility，也不能替代 archive evidence。

**HashKey release eligibility**:
只有同一 commit/lock/release identity 的 source phase 通过、七个 target 的 archive phase 全部通过、HSK metadata 与 tag/commit 一致，并且 Docker 与 gated `finalize-release` 生命周期在这些证据之后成功，才允许创建/填充 draft。Stable draft 仍由 maintainer publish，nightly 只能在 finalize 后自动 publish；任何 pre-existing release 都拒绝复用。
_Avoid_: Focused-tests-only release, OUT_DIR-only smoke, pre-gate draft reuse, undocumented known failures

**B20 activation schedule**:
由 activation timestamp 与 activation admin 组成的权威配置对，用于确定 B20 从何时开始生效。
_Avoid_: Current activation state, latest-chain inference

**B20 activation snapshot**:
在创建一次 EVM execution 时，根据其 timestamp 与 B20 activation schedule 固定该次执行是否提供 B20 precompiles；同一执行中的 `vm.warp` 不追溯改变该快照。
_Avoid_: Dynamic activation, per-call activation

**Feature admission state**:
Activation Registry 中控制新 B20 token 创建或 Policy Registry mutation 是否被接纳的链上状态；它不表示既有 token 被暂停。
_Avoid_: Feature kill switch, global pause

**B20 rollout plan**:
某条链在 B20 activation 之后，由治理权限执行 feature admission state 变更的权威有序交易计划。
_Avoid_: Automatic activation, timestamp-derived feature state

**B20 singleton**:
B20 Factory、Activation Registry 或 Policy Registry 这类位于固定地址的原生协议入口，与 Factory 创建的动态 B20 token 相区别。
_Avoid_: Deployed B20 token, singleton contract

**B20 code marker**:
写入原生 B20 地址账户 code 的单字节 `0xef` sentinel，使 Solidity 的 `EXTCODESIZE` / `address.code.length` 将该地址识别为可调用入口。standalone local genesis 只预置三个 B20 singleton marker；动态 `0xb2...` token marker 只能由 `B20Factory::createB20` 在同一 journal checkpoint 内原子创建，失败时随创建流程一并回滚。
_Avoid_: B20 implementation bytecode, arbitrary dynamic-address prewarming

**B20 protected state**:
不得由 `vm.store` 或 `vm.etch` 绕过原生 B20 规则修改的状态，包括三个固定 singleton，以及同时满足 canonical B20 地址结构与 `0xef` code marker 的已初始化动态 token。`vm.load` 保持可读；没有 marker 的未初始化 `0xb2...` 地址不属于受保护状态。
_Avoid_: Protecting the entire `0xb2...` namespace, blocking state inspection

**B20 snapshot state**:
通过现有 EVM journal/database snapshot 机制保存和恢复的全部 B20 mutable state，包括动态 token marker、token storage、Activation Registry 与 Policy Registry storage。B20 不维护独立的 snapshot side channel；固定 singleton marker 与初始 feature seed 属于 standalone genesis baseline，回滚到其后的 snapshot 时仍保留。
_Avoid_: B20-specific snapshot store, configuration snapshot

**B20 precompile inventory**:
通过现有 Anvil `eth_config.current.precompiles` 暴露的、当前 execution context 中可枚举的固定 B20 singleton 名称与地址。动态 `0xb2...` lookup domain 不可穷举，不进入静态 inventory；本地 B20 支持不新增 `hsk-cast precompiles` 命令。
_Avoid_: Dynamic token registry, execution prerequisite

**B20 trace identity**:
HashKey profile 的当前 EVM activation snapshot 启用 B20 时，trace 对三个固定 singleton 使用稳定的 `B20Factory`、`B20ActivationRegistry`、`B20PolicyRegistry` 标签，并将实际调用的 canonical 动态地址按 variant 标记为 `B20Asset` 或 `B20Stablecoin`。动态标签不读取 token name/symbol；其他 profile 中的相同地址仍按普通地址处理。
_Avoid_: Metadata-derived labels, global address-only detection

**B20 trace decoding**:
在 B20 trace identity 已成立后，使用 semantic baseline revision 提供的 canonical Factory、Activation Registry、Policy Registry、Asset 或 Stablecoin ABI 解码 calls、returns、events 与 custom errors。ABI 不匹配或数据 malformed 时保留原始 bytes，不查询在线 selector 服务，也不尝试其他网络或 variant 的 ABI。
_Avoid_: Online signature inference, cross-variant guessing

**B20 semantic acceptance**:
由两层证据共同组成：binding pin 对应 revision 的四套 canonical Asset、Stablecoin、Factory、Policy golden suites 原样守护 B20 业务语义；Foundry 只维护覆盖 runtime seam 与 CLI projection 的精简 conformance cases，不复制上游 golden suite 或另建一套 golden constants。
_Avoid_: Foundry-owned B20 semantics, duplicated golden corpus

**B20 observable execution result**:
Foundry conformance 对选定 operation 精确比较 success/revert status、原始 return/revert bytes、B20 native call gas used、logs 的 address/topics/data/order、B20 账户的 code marker/nonce/normalized storage diff，以及 Factory 的确定性 token address。MPT state root、block hash 与 output root 由 op-reth/Kona parity 覆盖，不属于 Foundry local backend 的验收输出。
_Avoid_: Decoded-text-only assertions, Foundry-generated consensus artifacts

**B20 conformance projection**:
共享 Core EVM 对代表场景执行 normal/inspected 精确等价验证；Forge 与 Anvil 各覆盖完整 stateful vertical slice，Cast 和 Chisel 只验证其复用执行语义的投影。各 CLI 不复制 upstream golden corpus，但整体至少覆盖 Factory、Asset、Stablecoin、Policy/Activation 的成功或失败路径。
_Avoid_: Per-CLI golden suite, smoke-only core coverage

**B20 activation boundary fixture**:
仅供内部 Rust/conformance tests 使用的 deterministic profile，例如 `activation_time = 100`，用于验证 `99` 未启用、`100` inclusive activation、`101` 持续启用，以及 normal/inspected 等价和 EVM-creation snapshot。它不形成用户可见 override；standalone HashKey profile 仍固定为 `activation_time = 0`。
_Avoid_: User-facing test knob, mutating the current EVM map after `vm.warp`

**B20 execution diagnostic**:
不改变 primary execution result 的 B20 可观察信息。成功启用 profile 不打印 banner；native revert bytes 原样传播并仅在 verbose trace 中按 canonical ABI 解码。Profile resolution、singleton collision 或 lookup composition failure 在执行前 fail closed，以 stderr typed error 报告 network、地址和原因；`--json` 与 verbosity 不改变 stdout 主结果。
_Avoid_: Rewritten native errors, stdout status prose

**B20 local simulation**:
Foundry 在 standalone EVM 中使用确定性开发状态执行调用 B20 native precompile 的 Solidity 合约；它不表示对生产 HashKey 链状态的复刻。
_Avoid_: Compiling the B20 implementation, production-chain emulation

**B20 local genesis state**:
HashKey B20 profile 在 standalone、non-fork execution 中一次性建立的开发初始状态：B20 从 timestamp `0` 起可用，以 `0xCB00000000000000000000000000000000000000` 作为 development admin，并预启用 B20Asset、B20Stablecoin 与 PolicyRegistry feature admission。后续普通、inspected 或 existing-context EVM creation 只继承当前 backend/journal，不重播 seed；只有重新建立 standalone genesis 才恢复该 baseline。
_Avoid_: HashKey genesis state, production rollout state

**B20 semantic baseline**:
定义 B20 业务语义与 golden suites 的权威上游 Git source 和不可变 revision；兼容绑定不得改变该 baseline 的 Rust 语义源码。
_Avoid_: Consumable dependency revision, moving upstream branch

**B20 binding pin**:
Foundry Cargo 实际消费的唯一、不可变兼容 revision；它派生自 B20 semantic baseline，只允许依赖 manifest 对齐，不拥有 B20 业务语义。
_Avoid_: B20 semantic fork, selectable B20 version, moving compatibility branch

**Resolved network profile**:
已结合显式配置、链身份和发布事实而确定，在一次 Foundry execution 中固定 EVM family、network extension 及其权威运行时配置的 network profile。
_Avoid_: Partially resolved config, inferred flags

**EVM construction**:
Foundry 从 resolved network profile、当前 execution environment 与可复用 execution state 建立一次语义一致的 EVM execution；该 execution 的 network semantics 在创建时固定。
_Avoid_: Profile transport, caller assembly, backend construction
