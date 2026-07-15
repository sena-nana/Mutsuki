# Mutsuki 架构设计

Mutsuki 的当前根级实现收敛为领域中立的 **Tick-first / Batch-first runtime**。

核心定位：

```text
Core = TaskStore / TaskPool + TaskLease + RunnerRegistry + Executor dispatch
     + ResultRouter + StateStore + ResourceManager + EventLog + TraceLog
```

Core 只保证 Task 作为外部语义单位进入 TaskPool，并在 tick 内被组织为
`WorkSet -> WorkResourcePlan -> WorkBatch` 后执行和提交。复杂流程、顺序执行、广播、
pre/filter/post、matcher 分派和业务 pipeline 均由协议包或编排插件在 Core 之上实现。

## 1. 分层

```text
RuntimeProfile + PluginManifest
  -> external/native resolver
  -> RuntimeLoadPlan / RuntimeLock
  -> HostRuntime control plane / CoreRuntime boot
  -> protocol task submit_batch / one-entry submit facade
  -> TaskStore / TaskPool
  -> SchedulerTick / WorkSet
  -> WorkResourcePlan
  -> WorkBatch + TaskLease
  -> RunnerDispatch
  -> RunnerExecutor invokes Runner.run_batch
  -> RunnerCompletion
  -> ResultRouter / StateStore / ResourceManager
  -> Rust SDK RuntimeClient / TaskHandleFuture wrapper
```

依赖方向：

- `contracts` 只定义 serde 纯协议对象。
- `core` 依赖 `contracts`，只实现 runtime mechanics。
- `host` 依赖 `core + contracts`，提供 `HostRuntime` 控制面门面、runtime bootstrapper
  和 JSONL runner client。
- `sdk` 依赖 `core + contracts`，只提供 Rust 插件作者侧 awaitable 包装。
- 外部 Python runner kit 镜像 contracts，提供 Python runner backend 和 stdio runner
  server；Rust crates 不依赖 Python。

分布式部署的零侵入依赖方向、禁止类型、Worker 本地化和旧插件 `LocalOnly` 默认行为由
[distributed-zero-intrusion-boundary.md](distributed-zero-intrusion-boundary.md) 冻结。
`MutsukiDistributedHost` 只能位于普通 Host Runtime 外层，Core、contracts、SDK 与普通
Host 不得反向依赖集群、共识、跨机 transport 或节点信任实现。

版本化任务录制、checkpoint 和内容寻址属于独立可选 contract surface，规则见
[portable-recovery-contracts.md](portable-recovery-contracts.md)。它们不进入默认 Core 热
路径；外部 Host/工具把 portable task 或 checkpoint 恢复为普通本地 Task，把 portable
resource materialize 为本地 ResourceRef。

通用执行策略、异构 backend variant 和固定容量性能画像同样是独立可选 contract surface，
规则见 [execution-policy-contracts.md](execution-policy-contracts.md)。单机 Host 可用本地
capability facts 选择 runner/variant；Core 不收集画像、不启动 sampler，也不把选择扩展为
全局 placement。

## 2. Protocol

`Protocol` 只表示数据契约：

- `protocol_id`
- input / output / error schema
- version
- ABI-safe codec
- compatibility rules

Protocol 不定义默认 pool、默认 task type、single/broadcast、pre/post、runner 选择、
overflow policy 或业务 workflow。

## 3. HandlerBinding

`HandlerBinding` 是插件声明自己可以处理某个 protocol 的逻辑消费绑定。它说明：

- 哪个 plugin 可以消费哪个 protocol。
- 该绑定目标 protocol / runner hint / pool。
- 可选 priority / policy / metadata。

编排器查询 `HandlerBindingRegistry`，再显式创建 targeted task。Core 不因为新增
binding 自动 fan-out，也不把一个 Task 交给多个 runner。

## 4. Task 与 TaskPool

`Task` 是 Core 中唯一可调度对象，是某次 protocol 处理请求生成的唯一状态机实例。

Task 具有：

- `task_id`
- `protocol_id`
- `input_refs`
- `output_ref`
- `continuation_ref`
- `target_binding_id`
- `lease_id`
- `trace_id` / `correlation_id`
- `registry_generation`
- `dispatch_lane`
- `ordering`
- `resource_requirements`

SDK-facing `TaskHandle` 只是 task id、protocol、target binding、trace/correlation 和
取消策略的 descriptor。Core API 可以返回 handle，但 Core 不把它解释成语言级 Future。
Core 的公开 facade 在 submit/status/result/outcome/events/cancel/wake 上接受该
descriptor；字符串 task id 只保留为 TaskPool 和 host actor 内部事实键。

`TaskPool` 是 ready task backlog / 调度索引，不是 Runner inbox。没有 runner 时，
Task 仍然可以保持 Ready 并积压。Runner 不长期拥有 Task。

TaskPool 内部以 TaskRecord 为权威，同时维护可重建的增量索引：protocol/hint/owner 复合稳定
ready queue、Waiting/Blocked wake bucket、Running lease-expiry bucket、expected-version
ready set 和 runner running/waiting set。正常 tick、runner load 和 claim 只访问本轮到期桶、
相关 protocol/runner selector 队列与实际候选；完整 TaskRecord 扫描只允许出现在显式
snapshot、occupancy audit、Abort 或 reload index rebuild 等非调度热路径。

Terminal TaskRecord 默认无限保留，以保持既有 TaskHandle outcome 和 runtime 生命周期内
task id 唯一语义。长期 Host 可以显式启用 `TaskHistoryRetention`：Core 按 created sequence
淘汰超过上限的 terminal record，同时保留一个独立有界的 evicted task id 防重窗口；仍被
parent continuation 引用的 child record 在 wait link 消费前受保护。淘汰后的旧 handle 查询
结构化返回 task not found；防重窗口过期后 task id 可以再次使用。要求永久 outcome 或永久
幂等键的产品必须把历史放在外部持久层，或保持默认无限策略，不能把 Core 内存当归档库。

每次 claim 都创建唯一的本地 attempt：TaskPool 单调递增 `attempt_generation` 并将其
编码进既有 TaskLease id。Cancel、retry、lease expiry、reload 和 Abort 都通过 active
lease fencing 拒绝迟到结果，不增加 distributed/global lease 概念。单 Host 的
Cancel/Drain/Abort、非阻塞事件和统计契约见
[local-task-lifecycle.md](local-task-lifecycle.md)。

Core 保留单 Task 状态：

```text
Created / Ready / Running / Waiting / Blocked / Completed / Failed / Cancelled / Expired / DeadLetter
```

`Waiting` 和 `Blocked` 只描述当前 Task 自身状态，不表示等待一组 task、workflow
stage 或 broadcast completion。

## 5. WorkBatch / TaskLease / Executor / Runner

Core 的唯一执行语义：

```text
多个 ready Task -> WorkBatch(entries) + TaskLease[] -> 一个 Executor 调用一个 Runner
```

`Runner` 是插件提供的逻辑处理器，负责推进一个 task step 并返回结果。Runner 不持有
Task 权威状态、workflow 状态、连接池、长期 stream、lock 或 transaction。

`Executor` 是物理执行槽位，可以是线程、进程、Wasm instance、沙箱、远程 worker 或
插件运行实例。Executor 可销毁；销毁不应导致 Task 状态、Continuation、连接池或资源
状态丢失。

当前 core 已把 runner 调用预拆成 `RunnerDispatch -> RunnerExecutor ->
RunnerCompletion`。默认 `InlineRunnerExecutor` 仍同步调用 runner，用于测试、replay
和最小 host；默认 `HostRuntime` 在 host crate 中启动 CoreActor 控制线程，并将
`RunnerDispatch` 投递给两个有界多消费者 pool：Orchestration / Io / Cpu 共享 compute
pool，Blocking / Script 共享 bounded blocking pool。线程模型不进入 core crate；core
只保留 claim、lease fencing 和 completion route 的事实源。

调度策略第一阶段位于 host 控制面：`SchedulerPolicy` 读取 runner descriptor、
`RunnerLoad`、runner limits、`HostCapacity` 和 worker pool capacity，返回本轮
`DispatchBudget { max_entries, max_batches, max_bytes, lane_budget }`。Core 只接受被 host
clamp 后的 budget，并继续通过 TaskPool 排序、lane budget 筛选、claim、TaskLease
和 route_result 维护唯一事实源。scheduler 不能选择具体 task、不能调用 runner、不能
修改 task 状态，也不能访问资源本体。

HostCapacity 包含物理 pool 的 batch 与 entry 两级 running / queued 计数、active worker、
saturation、preferred batch size、max entry concurrency 和剩余 inflight bytes。队列容量由
bounded channel 的 `try_send` 原子判定，不能用旁路计数先判断再发送。Core 构造 WorkResourcePlan 时
给出 parallel_groups、serial_groups 和 parallelism_limit；Host / SDK scalar adapter 只能
在 scheduler budget、HostCapacity、runner capability 和 resource plan 都允许时有界并行。

当前执行模型明确采用单实例串行 Runner：每个逻辑 `runner_id` 同时最多一个 active
`WorkBatch`。`RunnerDescriptor.batch.max_inflight_batches` 与 Host
`RunnerLimits.max_running` 都必须为 `1`，更大的配置在 load-plan materialize 或 Host
启动阶段结构化拒绝，scheduler 的 `DispatchBudget.max_batches` 也会 clamp 到 `0..=1`。
该限制只约束 batch 之间的并行；一个 `WorkBatch` 仍可包含多个 entry，
并按 `max_entry_concurrency` 与资源计划进行 batch 内有界并行。

Host runner limit 中，`max_inflight` 只统计 Running 与 Waiting entry；Ready backlog 是
TaskPool 中等待 claim 的供给，不占用 inflight，也不能反向阻止自身出队。Host 的
`pool_queue_limit` 只限制物理 worker dispatch 队列，不限制 TaskPool Ready backlog。

长期如果 scheduler provider / plugin 化，关系仍保持：

```text
Core owns state.
Scheduler reads snapshot.
Scheduler returns decision.
Core validates and commits claim.
```

该长期形态不得把 protocol 编排、广播、pre/filter/post 或业务 QoS 语义放回 Core。

`ExecutionClass` 是 host 选择物理执行池的提示：

```text
Control / Orchestration / Io / Cpu / Blocking / Script
```

`Control` 只用于 core kernel 控制任务，不表示普通插件可在 CoreActor 中运行。
`Blocking` 和 `Script` 默认进入独立 pool，避免 FFI、外部进程或脚本解释器污染
CoreActor 调度延迟。

当前 Rust core 的 `Runner.run_batch` 使用 `WorkBatch` wire shape；单 task 只是
`entries.len() == 1` 的 batch。host 可在同一 tick 为不同 runner 产生多个 batch dispatch，
但同一 `runner_id` 只能有一个 active dispatch。每个 dispatch 携带该 batch 的 entry、
payload、resource plan 和 task leases。
`RunnerDescriptor.batch.mode` 只声明实现形态：`native_batch` 可在 runner 内做原生 batch
优化，`scalar_adapter` 表示 SDK/Host adapter 串行把每个 entry 降到作者侧 scalar 函数；
两者对 Core 暴露的 ABI 都仍是 `run_batch`。

SDK / host submit 面也保持 batch-first：`submit_batch(TaskBatch)` 是标准入口，
`submit_task` / `submit_one` 只是构造 one-entry `TaskBatch` 的用户体验 facade。ABI
后端必须使用 `task.submit_batch`；不得为旧 single-task 提交流程保留独立 wire method。

Rust SDK 的 `ctx.call(...).await` 编译为：

```text
生成 child protocol task
RunnerResult.tasks 显式提交 child
RunnerResult.task_await 注册 parent -> child wait link
parent task 进入 Waiting 并释放 TaskLease
child terminal event 唤醒 parent continuation
```

这只是 SDK 语法糖；Core 不暴露 `async fn`、`Future` ABI、language executor、
`join_all`、`select`、`TaskGroup` 或 `WaitSet`。

SDK 同时提供 Host / Plugin 扩展基础抽象，但这些抽象只服务于 host-side composition
和插件作者 API：

- `PluginBuilder` / `PluginLoader` 构造 boot 前 `PluginManifest`、runner 和 host service
  声明；它们不绕过 resolver、load plan 或 registry freeze。
- `HostContext` 聚合 active `CapabilityBroker`、host-only `HostServiceRegistry`、
  `ConfigProvider`、`EventBridge`、`TaskSubmitter`、`ResourcePlanGateway` 和
  `ShutdownController`。
- `TaskSubmitter` 和 `ResourcePlanGateway` 只执行现有 `Task`、`TaskHandle`、`TaskOutcome`
  与 resource plan 协议；builtin/ABI 等部署差异仍停留在 host backend。
- `HostRuntime` trait 是 SDK-facing host control-plane facade，可暴露 reload、
  task snapshot、event、trace、Drain/Abort 和统计查询；reload 的 prepared transaction 由具体 host
  通过关联类型承载，SDK 不反向依赖 host crate，也不新增 contracts wire shape。

这些 SDK trait 不新增 contracts wire shape，也不把 Host shell 或 SDK 本身变成运行时插件。

当 runner 返回 Waiting 时，当前 task 释放 OS worker 和 `TaskLease`，但 task record
保留 `owner_runner`。后续 wake 只让原 runner reclaim continuation；waiting task 仍计入
runner inflight，用于 backpressure，防止等待中的父 task 无限堆积。

HostRuntime cancel 是控制面消息，不中断 CoreActor。对于正在 worker 中运行的 task，
CoreActor 先将 task 标记为 cancelled，并记录 runner 级 pending cancel；worker 返回
`RunnerCompletion` 时，host 在归还 runner 前通过 `Runner.cancel(invocation_id)` 投递。
`RunnerLimits.deadline_ticks` 可为 invocation 生成 tick deadline，超期时走同一
cancel propagation 路径。Host-only wall-clock deadline、取消宽限和 worker health
timeout 不改变 `RunnerContext` wire shape；它们由 actor 监督 running invocation。
native worker 超时后只会进入 cooperative cancel / quarantine；原线程退出前禁止补充
replacement。隔离数达到上限后 pool degraded 并拒绝新 dispatch，迟到 completion 只投递
cancel / dispose，不再复用旧 runner 或提交旧结果。process runner 暴露独立 termination
handle；hard timeout 会 kill 子进程，使阻塞 JSONL 调用返回，随后重建 process runner，且
仅在原 worker 真实退出后补充 worker capacity。

HostRuntime 的 event-driven driver 复用同一 CoreActor mailbox：外部 submit 与控制命令、
worker completion 和 shutdown 都是唤醒事件。TaskPool 维护 future-ready、waiting wake 和
lease expiry 的 step index，并只公开最近 `next_required_step`；actor 将它与 runner tick
deadline、wall-clock deadline、cancel grace 和 worker health deadline 合并为一个最近截止
时间。到期时 Core 可直接前进到该逻辑 step 并执行一次完整调度校验，禁止用逐 step 空 Tick
追赶。无截止时间时 actor 阻塞等待 mailbox，不启动采样或 polling 线程。

Host shell 等待 task 完成时订阅 host-local terminal revision。revision 允许合并多个完成，
只作为唤醒提示；消费者醒来后用一条 batch command 按 `TaskHandle` 读取权威 status/outcome。
订阅关闭或 actor 退出必须唤醒等待方，禁止用无界 completion queue、逐 task polling 或第二份
terminal 状态替代 TaskPool。

Drain 只拒绝新的外部 submit，已经接收的 task 继续完成；Abort 取消所有非 terminal task
并永久使旧 lease 失效。生命周期事件进入有界 drop-new EventLog，容量满或设为零只影响
观察，不影响任务状态迁移。统计由 actor-owned 状态迁移常数成本累积，不启动采样线程。

EventLog 与 TraceLog 都由 `RuntimeProfile.observability` / `RuntimeLoadPlan.observability`
确定容量和 drop-oldest/drop-new 策略，并以全局单调 sequence 提供有界 cursor page。
默认 scheduler decision 只累加 `RuntimeStatistics.scheduler_decisions`，不创建 attrs/event/span；
逐 decision 明细和逐 dispatch span 分别由显式开关启用。trace capacity 为零时不分配持久
容器，lazy record closure 不执行。retained/dropped event/trace 统计只描述观察面，不能参与
Task 正确性。

外部 Python runner kit 的 `await ctx.call_raw(...)` 使用同一 wire shape：runner-side
adapter 只在 coroutine yield 出 Mutsuki `TaskAwait` 时暂停并返回
`RunnerStatus::Waiting`。它不把 `asyncio` event loop、任意 Python awaitable 或调度器
语义写入 Core。

## 6. ResultRouter

RunnerResult 不直接修改事实源：

- completed / failed / cancelled / waiting / blocked 更新当前 Task。
- `task_await` -> 保存当前 task continuation，注册 child terminal wake link。
- `deltas` -> `core.commit` task -> Committer runner -> StateStore。
- `events` -> `core.event.append` task -> EventLog。
- `effects` -> `effect.*` task -> Effectful runner。
- `tasks` -> 显式进入 TaskPool。
- `values` / `resources` -> Resource/Event lineage facts。

复杂编排必须由插件 runner 显式返回后续 task 或监听 TaskEvent 后再提交 task；Core
不提供 TaskGroup、WaitSet、pipeline.run、broadcast.run、matcher.run 或 actor.send。
当 `Waiting` 来自 SDK await，Core 只维护单个 child task 到 parent continuation 的
唤醒索引。

## 7. ResourceManager

控制面经过 Core，数据面不一定经过 Core。TaskPool 不搬运大数据本体。

资源和值统一以 descriptor 表达：

- `ValueRef`：小型结构化、可共享、可版本化值。
- `ResourceRef`：大型数据、blob、file-backed mmap、stream 或 provider-RPC 资源。
- `ResourceCellRef`：长期资源状态单元，例如连接池、stream、cookie jar、rate limiter。
- `ResourceLease`：某个 task step 临时使用 ResourceCell 的租约。
- `StateRef`：跨 task 权威语义状态。

默认规则：

- 共享资源 readonly / sealed。
- 修改产生新 ref 或 StateDelta。
- 原地写必须 ExclusiveWriteLease。
- Runner 可以持有短期 ResourceLease，但不能拥有 ResourceCell。
- 短期可变 lease 默认不能跨 SDK await；需要长期持有必须在更高层声明
  LongLease / Transaction / PinnedResource 等显式机制。

当前 `ResourceManager` 只保留 descriptor、lease、occupancy 和 generation 的事实源。
file/blob/mmap 读写、snapshot/patch/export/query 等数据面由 host resource backend /
resource provider 执行，不进入 Rust core。

### 7.1 ResourceHub 与 Typed Store

资源平面按 `ResourceHub -> Typed Store -> backend/provider` 分层：

- `ResourceHub` 负责 descriptor/generation/lease 校验、surface occupancy、trace route
  和 provider operation 分发所需的 registry 事实。当前 core 实现使用单一 descriptor
  表，`ResourceSemantic` 只用于路由标签和占用解释，不表示 Core 内已有完整 typed
  backend。
- Typed Store 按资源语义保存与优化本类资源，例如 `FrozenStore`、`SnapshotStore`、
  `FactStore`、`CowStore`、`CapabilityStore`、`StreamStore` 和 `TransactionStore`。
  这些是 host resource backend / provider 的实现边界，不是 CoreRuntime 默认职责。
- backend/provider 负责真实数据访问；builtin、ABI、WASM、process 的差异只存在于
  host/resource backend，不进入插件公共 API。

公共插件 API 只能使用 `ResourceClient`、`TypedResourceHandle` 和可序列化的
`ReadPlan` / `WritePlan` / `StreamPlan` / `ExportPlan` / `CommandPlan`。不得暴露
`Arc<T>`、`&T`、`&mut T`、`downcast` 或 `with_native_*`。

资源语义分类：

- `FrozenValue`：创建后不可变，可 content-addressed、dedupe、LRU。
- `VersionedSnapshot`：不可变版本化快照，可 stale 使用，用于回放、分析和 patch base。
- `ReadOnlyFact`：Host/特权组件可刷新，普通插件只读，带 generation/provenance。
- `CowVersionedState`：可变状态通过 snapshot + patch/transaction 提交，版本冲突 fail-loud。
- `CapabilityResource`：能力句柄，只能通过 command plan 操作，不可 snapshot/export native。
- `StreamResource`：分块、背压、取消和 pipe/collect 语义。
- `TransactionResource`：begin/stage/commit/rollback；外部副作用只能声明有限保证。

`ResourceId` 结构为 `kind_id / slot_id / generation / version`。`ref_id` 作为兼容路由键
保留，但 `ResourceRef.resource_id.generation/version` 必须与 `ResourceRef` 顶层字段一致。

懒计划规则：

- 构造 `ReadPlan` 不访问资源；`eval` / `collect` / `snapshot` / `open_stream` 才执行读。
- 构造 `WritePlan` 不修改资源；`commit` 才执行写并检查 `base_version`。
- 写后读必须显式使用 `returning` 或基于 commit receipt 的新版本重新创建读计划。

## 8. Plugin Loading

插件声明能力，RuntimeProfile 决定组合，resolver 生成确定性 load plan，Core 只校验和
物化。

RuntimeProfile 同时声明每个 enabled plugin 的部署形态。Builtin、ABI、WASM、
process 和 Python 都是 Host 执行面后端：同一插件能力 surface 必须通过统一
TaskClient / ResourceClient / runner 协议暴露，插件业务代码不根据部署形态分叉。Host
可以为 builtin 注册本地 runner，也可以为 ABI/WASM/process/Python 注册 bridge runner，
但这些差异不能进入 Core 调度语义或公共插件 API。

RuntimeProfile 还声明发行 profile mode。`FullDev` 和 `ExtensibleRuntime` 以可调试和
可外部扩展为主，resolver 保守保留 manifest 声明的 host extension surface。
`BuiltinOnly` 和 `LockedBuiltin` 可根据 enabled builtin plugin、deployment kind 和
manifest `requires` 生成 active capability graph；Host 只注册并声明 graph 中激活的
backend、codec、bridge、scheduler policy 和 workflow。裁剪对象是实现和注册路径，
不是 Task、ResourceRef、ResourceRegistry、permission、lifecycle、cancel 或 trace
等 Core 抽象。

Core 不负责插件扫描、下载、安装、依赖解算、版本选择、Python/npm/cargo 依赖管理或
运行组合策略。

Core 负责：

- 校验 `RuntimeLoadPlan`。
- 校验 runner descriptor 不超出 lock 授权。
- 构建并 freeze RunnerRegistry、HandlerBindingRegistry 和 contract surface。
- 记录 registry generation、plugin generation 和 contract fingerprint。

与 GitHub issue #13 对齐后的职责边界是：Core 拥有 load-plan materialization、
registry / binding index、surface occupancy 和 generation 切换的事实源；Host / SDK
拥有具体 `PluginLoader`、builtin / ABI / WASM / process / Python bridge、路径发现和
运行环境适配。插件加载只能在 boot 或 prepared reload transaction 中生成新的
`RuntimeLoadPlan` / registry generation；Core v1 不提供运行中 lazy load 后动态注册 runner
或 provider 的入口。

### 8.1 运行时可替换边界

Core kernel 不是插件，Host shell 也不是整体插件。Core 固定 Task 状态机、TaskLease、
RunnerRegistry、Resource Registry、Resource 生命周期、权限入口、cancel/trace 基础事件、
ID/generation 规则和 hot reload 事务边界。Host 固定为 runtime 容器、插件加载器和
生命周期协调器。

可替换对象必须是稳定接口后的 host service / backend 或普通业务能力：

- 业务插件：runner、provider、protocol、workflow、resource provider。
- Host backend：builtin / ABI / WASM / process / Python bridge、codec、trace sink、
  resource backend、permission policy、scheduler policy。

SDK 不是运行时插件。SDK 可以拆 crate 或语言包，但它只定义插件作者 API 和语法糖；
运行时语义必须落回 Task、TaskHandle、ResourceRef、plan 和 RunnerResult。Shim 分两层：
host-side bridge 可以作为 Host backend 替换；guest-side shim 随插件 artifact 版本走，
不由 Host 单独热替换。

Host backend / plugin backend 通过 `HostExtensionDescriptor`、`PluginBackendDescriptor`
和统一 `TaskClient` / `ResourcePlanClient` 边界表达。Builtin 快速路径和 ABI/JSONL 路径
只能是不同 backend 实现，不能把 `Arc<T>`、`&mut T`、SDK client 或 native handle 暴露给
插件业务代码。

当前实现已完成 Host backend 裁剪的 Level 2 boot gate：resolver 从 enabled plugins、
deployment 和 requires 生成 active graph，并只把 active backend / bridge / codec /
scheduler policy / workflow 放入 contract surface；RuntimeBootstrapper / host boot 再按
`RuntimeCapabilityGraph.active_*` 注册或拒绝 plugin backend、bridge、codec 与 scheduler
policy。active backend 引用的 bridge deployment 和 codec 支持关系必须在 CoreRuntime
启动前校验；host config 中配置的 scheduler policy 必须匹配 active scheduler descriptor。
完整 backend instantiation supervision、连接 drain / replacement 仍是后续工作。

ABI 动态库不定义第二套 runner/resource 方法面。Core SDK 只为动态库提供版本化的最小
bytes transport：固定入口建立 connection，Host 与 guest 互相发送既有 JSONL request /
response。首个 JSONL 请求必须是 `plugin.initialize({ config })`，配置来自与 builtin 相同的
owner-defined product config；初始化返回 manifest、codec、bridge 与 provider surface。
`runner.run_batch`、management cancel/dispose、TaskClient 与 ResourcePlanClient 继续使用同一
wire shape；平台动态库发现、校验、装载、drain 和卸载仍由具体 Host 负责。

同一业务插件的不同 deployment 通过 `PluginManifest::business_surface` 比较。该 surface
排除 artifact、lifecycle 与 codec/bridge/backend 等部署 transport，只保留协议、Runner、
Resource、workflow、requires 和 permission 等业务事实；配置不得改变该 surface。

Resource Registry、ResourceId 分配、lease 基础规则和 owner 路由事实属于 Core /
ResourceManager；`ResourceProvider`、typed store backend、export/query/patch/stream
handler 可以替换。provider 热替换必须按 `ResourceProviderReloadPolicy` 与
`ResourceProviderCompatibility` 判定：无 live resource 可直接替换；有 resource 但无 active
lease 时必须保持 kind/schema/operation/generation 兼容；存在 active lease、stream 或 mutable
transaction 时只能 drain、cancel、freeze、migrate 或阻断。

Scheduler 只插件化 policy，不插件化 scheduler engine。`SchedulerPolicy` 只能读取 snapshot
并返回 dispatch budget，Core 仍执行 TaskPool 排序、claim、TaskLease fencing 和状态提交。

Workflow 应作为普通 runner/plugin 实现；实例状态必须资源化，例如
`WorkflowInstanceResource` 保存 current step、child task、waiting set、retry count、cancel
state 与 trace id。Codec/bridge 在连接握手时确定，一个连接生命周期内固定；新 codec 只用于
新连接，旧连接 drain 后释放旧 codec。

## 9. Hot Reload

热重载使用新 plugin generation，不原地替换对象。

Contract surface 兼容性：

- `Identical`：直接热重载。
- `Additive`：可热重载。
- `Deprecated`：可保留兼容处理，但禁止新增占用。
- `Removed`：必须 zero occupancy。
- `Breaking`：必须 migration、drain 或 restart。

切换 active generation 后，新 task 使用新 registry；旧 generation 不原地替换，也不在
存在污染/未知 running invocation 时提前 dispose。

## 10. Domain Neutrality

业务对象不是 runtime 实例，而是上层 Store 中的数据聚合。Rust core 中不得出现领域或
产品专用执行分支。
