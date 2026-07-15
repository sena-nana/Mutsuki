# Mutsuki Runtime Contracts

根级 Rust contracts 是当前协议事实源。外部 Python runner kit 必须镜像这些 wire shape。

## 1. 协议总则

- 所有协议对象必须可 serde JSON roundtrip。
- 所有字段显式出现；缺字段反序列化失败。
- Core 只解释通用 runtime 字段，不解释业务 payload。
- 失败使用 `RuntimeError`。
- 禁止跨 runtime 边界传 callable、socket、SDK client、数据库连接、Python object、
  Rust pointer、Arc、Vec 本体或真实 handle。

## 2. 核心对象

| 对象 | 语义 |
|---|---|
| `Task` | 统一待处理控制消息，包含 protocol_id、priority、ready_at_step、payload、refs、target_binding_id、lease_id、expected_versions、registry_generation、dispatch_lane、ordering 和 resource_requirements |
| `TaskBatch` | SDK / host submit 面的 batch descriptor；只携带待入队 task 与可选 resource plan，不决定 runner dispatch payload layout；单 task submit 也必须包装为 one-entry batch |
| `TaskLease` | 一次 batch entry 执行租约，绑定 task、runner、executor、registry generation 和租约时间 |
| `PortableTask` / `TaskCheckpoint` | 可选的版本化 task 录制/重放与 plugin-owned checkpoint envelope；恢复后仍创建普通本地 Task |
| `ContentId` / `PortableResourceDescriptor` | 通用 digest/size/format 内容标识与待 materialize 资源描述，不携带部署位置 |
| `PortabilityCatalog` | 独立可选的执行移动性、重试安全、接收持久化和资源持久化声明；缺失时为 LocalOnly |
| `ExecutionPolicy` / `ExecutionOutcomeMetadata` | 显式 latency/deadline/质量/cache/partial/fallback 策略与实际执行结果元数据 |
| `CapabilitySet` / `RequirementSet` / `ExecutionVariantCatalog` | 本地架构、后端、精度、内存、版本能力匹配与同一 task type 的可选实现 |
| `ExecutionProfile` / fixed profile accumulators | Host 显式驱动的固定容量 latency/throughput/memory/failure 画像 |
| `BatchEntry` / `WorkBatch` / `CompletionBatch` | Tick 内部标准执行单元；单 task 是单 entry batch，不再有独立 runner step 主路径 |
| `BatchPayload` / `PayloadLayout` | `layout + payload` 形式的执行面 payload descriptor；Row 携带 `RowPayload.rows`，Columnar / BinaryPacked / ResourceBacked 是 SIMD-friendly 能力面 |
| `WorkResourcePlan` | WorkSet 派发前生成的资源读写计划、version check、parallel/serial entry group、parallelism limit 和冲突 entry 描述 |
| `TaskHandle` | SDK-facing task descriptor，包含 task id、protocol、target binding、取消策略和 trace/correlation |
| `TaskAwait` | 当前 task 等待一个 child task 的 continuation registration |
| `TaskOutcome` | SDK 读取的 terminal task 结果映射；Completed 可携带小型 inline output 或 provider-owned output_ref |
| `CancelPolicy` | SDK await 取消策略，当前默认支持 Cascade，Detach / Shield 为协议预留 |
| `TaskStatus` | created、ready、running、waiting、blocked、completed、failed、cancelled、expired、dead_letter |
| `ProtocolDescriptor` | protocol_id、schema、codec、version、compatibility 等纯数据契约 |
| `HandlerBinding` | 插件对 protocol 的逻辑消费绑定，指向目标 protocol / runner hint / pool |
| `RunnerDescriptor` | runner_id、plugin_id、generation、accepted_protocol_ids、purity、execution_class、schema、batch/payload/resource/ordering/control capability、metadata |
| `ExecutionClass` | host 执行池分类：Control、Orchestration、Io、Cpu、Blocking、Script |
| `RunnerPurity` | Pure、Committer、Effectful |
| `RunnerResult` | task_id、可选小型 output、deltas、events、tasks、effects、values、resources、status |
| `StateDelta` | target_ref、expected_version、patch、conflict_policy |
| `EffectRequest` | effect_id、kind、payload、preconditions、idempotency_key |
| `ValueRef` | 小型结构化共享值 descriptor |
| `ResourceRef` | 大型资源 / mmap / blob / stream / provider-RPC descriptor |
| `ResourceId` | typed store 路由 descriptor，包含 kind_id、slot_id、generation、version |
| `ResourceSemantic` | 资源语义分类：FrozenValue、VersionedSnapshot、ReadOnlyFact、CowVersionedState、CapabilityResource、StreamResource、TransactionResource |
| `ResourceProviderReloadPolicy` / `ResourceProviderCompatibility` | Resource Provider 热替换策略和兼容性声明 |
| `ResourceTypeDescriptor` | 插件声明的资源类型、语义、schema、provider、operation 和 provider 热替换兼容规则 |
| `ResourceCellRef` | 长期资源状态单元 descriptor，例如连接池、stream、cookie jar、rate limiter |
| `ResourceLease` | task step 临时使用 ResourceCell 的租约 |
| `LeaseToken` | ref_id、owner、mode、expires_at_step、generation |
| `ReadPlan` / `WritePlan` / `StreamPlan` / `ExportPlan` / `CommandPlan` | 可序列化资源操作计划，构造阶段不执行真实读写 |
| `SnapshotDescriptor` / `PatchDescriptor` / `PlanReceipt` | 版本化 snapshot、patch 与 provider plan commit receipt；receipt 只能携带 descriptor 更新和小型结构化输出，不携带资源 bytes |
| `TransactionPlan` / `CommandBatch` / `SagaPlan` | Experimental provider/workflow descriptor；CoreRuntime 不解释事务、批处理或 saga 执行语义 |
| `RuntimeProfile` | 本次运行启用哪些插件、发行 profile mode、绑定哪些能力、是否允许热重载 |
| `PluginDeploymentKind` | RuntimeProfile / RuntimeLoadPlan 中声明插件本次部署形态：Builtin、Abi、Wasm、Process、Python |
| `RuntimeCapabilityGraph` | resolver 从 enabled plugins、deployment、provides/requires 生成的 active capability 视图，用于 host 声明与裁剪一致性 |
| `CapabilityProviderSelection` | resolver 为 active capability 选择的 provider 插件、版本和 surface descriptor |
| `PermissionAuditEntry` | resolver 对插件 effect/resource 权限 grant 的结构化审计结果 |
| `PluginManifest` | 插件声明 runner、protocol、handler binding、resource schema/provider、effect、stream、subscription、timer、permission、lifecycle |
| `HostExtensionDescriptor` | Host 内部 backend/service 扩展点 descriptor，例如 bridge、codec、trace sink、resource backend、scheduler policy |
| `PluginBackendDescriptor` | 某部署形态的 task/resource client 后端绑定 descriptor |
| `CodecDescriptor` / `BridgeDescriptor` | 连接级 codec 与 host-side shim/bridge descriptor |
| `SchedulerPolicyDescriptor` | host 级 scheduler policy descriptor；只描述 dispatch budget 决策策略 |
| `WorkflowDescriptor` | Experimental workflow 插件与外置实例状态资源的绑定 descriptor；CoreRuntime 不拥有 workflow 实例状态 |
| `RuntimeLoadPlan` | resolver 生成的确定性加载计划和 registry generation |
| `ContractSurface` | runner/task/schema/resource/effect/stream/subscription/timer/lifecycle/permission 等热重载比较单元 |
| `SurfaceOccupancyHandle` | stream/subscription/timer 等 lifecycle 占用 descriptor |
| `RuntimeEvent` | sequence、kind、name、subject_id、attributes、error |
| `TraceSpan` | 单调 sequence、trace_id、span_id、parent_span_id、name、interval、attributes、status |
| `ObservabilityProfile` | event/trace outlet 容量、drop-oldest/drop-new 策略，以及 scheduler 明细和逐 dispatch span 开关 |
| `ObservabilityPage<T>` | cursor 分页结果，包含 next/earliest/latest sequence、lost、truncated 和累计 dropped |

Builtin Rust Host 通过 `HostContext` 同时公开 plan-only gateway 与 host-owned
`ResourceRegistryGateway`。后者按显式 `provider_id` 创建 blob、COW state 或
capability resource，并按 `ref_id` 打开 registry 中的最新 descriptor。边界只传
descriptor 与 bytes；provider 实例和 native handle 始终由 Host 持有。未知 provider、
stale descriptor 和未授权 surface 必须结构化失败。v1 不因此声明 ABI 或 Python
部署形态具备等价能力。

## 3. Task 与 Runner 接口

`Task.protocol_id` 是当前调度事实源。wire shape 不包含额外 task kind 兼容字段。

```text
Runner.run_batch(ctx, batch) -> CompletionBatch
Runner.cancel(invocation_id)
Runner.dispose()
```

JSONL runner ABI 只暴露 batch 方法面：

```text
runner.run_batch({ runner_id, ctx, batch }) -> CompletionBatch
runner.cancel({ runner_id, invocation_id })
runner.dispose({ runner_id })
```

Host / SDK 的 task submit 面同样以 batch 为标准入口：

```text
task.submit_batch({ batch: TaskBatch }) -> TaskHandle[]
```

`submit_task` / `submit_one` 只允许作为 SDK / host facade，它们内部必须构造
`TaskBatch::one(...)`，不得重新引入 `task.submit` 或 runner single-step ABI。

`RunnerDescriptor.execution_class` 只描述 host 物理执行池选择，不是业务协议语义。
`Control` 仅用于 core kernel 控制任务；普通插件不得因为声明 Control 而在 core 控制面
执行。

`RunnerContext` 至少包含：

- `registry_generation`
- `current_step`
- `executor_id`
- `tick_id`
- `batch_id`
- `task_lease_ids`
- `entry_count`
- `invocation_id`
- `cancel_token`
- `deadline_tick`
- `cancel_requested`

Core 记录 `runner.run_batch` trace span 时必须绑定本次 dispatch 的 batch / entry 事实：
优先使用首个 entry 对应 task descriptor 的 `trace_id`，缺失时使用 deterministic
`trace-task-<task_id>`；span attributes 必须包含 batch id、tick id、entry count、
task lease ids、executor id、payload layout，以及存在时的 correlation id。

Core 每个 tick 可按 scheduler budget 为同一 runner claim 多个 ready task，并构造为唯一
active 的 `WorkSet -> WorkResourcePlan -> WorkBatch` 后一次交给 Runner。当前每个逻辑
`runner_id` 同时最多一个 active batch；并行只发生在该 batch 的 entry 内，或不同 runner
之间。每个 runner completion
必须返回对应 `CompletionBatch`，其中每个 `EntryCompletion` 必须可唯一映射到本 batch
的 entry / task lease。缺失、重复或未知 entry 都必须在写入任何输出事实前结构化失败为
`task.claim_conflict`，并终止对应 leased task，不能依赖 lease expiry 伪装为可重试。

Event/Trace 消费以 `sequence` 表示“调用方已处理的最后一条记录”，查询只返回更大的
sequence，并受显式 `limit` 限制。`ObservabilityPage.next_sequence` 是下一次查询应提交的
cursor；`lost > 0` 表示该 cursor 与返回位置之间存在已淘汰或 drop-new 的记录，
`truncated = true` 表示仍有未扫描区间。空结果也可以推进 `next_sequence` 跨过已丢失
区间，避免消费者永久停留在不可恢复的 cursor。

SDK 层可以把 task 原语包装成语言 awaitable。当前仓库内 issue #5 的 Rust 侧落点是
Rust SDK；Python runner kit 位于独立仓库。JS/TS SDK 只作为同一
`TaskHandle` / `TaskOutcome` wire shape 的后续外部 SDK 目标，不在当前 workspace
新增占位包。

```text
Rust SDK: ctx.call::<Protocol>(input).await -> TaskOutcome
Rust SDK raw: ctx.call_raw(protocol_id, payload).await -> TaskOutcome
Python runner kit: await ctx.call_raw(protocol_id, payload) -> TaskOutcome
JS/TS SDK: future package 可包装同一 TaskHandle / TaskOutcome wire shape
```

任务持久化、重放、checkpoint 与内容寻址使用独立可选 contracts，不扩展 task execute ABI。
现有 `Task` / `TaskHandle` / `TaskLease` 的 portable 审计、流式编解码与恢复规则见
[portable-recovery-contracts.md](portable-recovery-contracts.md)。

Rust SDK 的 derive / attribute 宏只生成同一 `ProtocolDescriptor`、`ResourceTypeDescriptor`、
`RunnerDescriptor`、`PluginManifest` 和 `AsyncRunnerAdapter` glue；它们不是新的 wire
protocol object，也不能引入 workflow、broadcast、本地直调或绕过 TaskPool 的执行语义。

Batch-first 迁移约束：

- 旧 `Runner.step(ctx, task) -> RunnerResult` 实现必须迁移为
  `Runner.run_batch(ctx, batch) -> CompletionBatch`。
- 标量 runner 可通过 SDK / host adapter 顺序遍历 `WorkBatch.entries`，逐 entry 产出
  `EntryCompletion`；这只是兼容执行形态，不是 single-task ABI。
- 当前 v1 Core dispatch 只生成 Row payload，因此可执行 runner 必须在
  `RunnerDescriptor.payload.layouts` 声明 Row；支持同构输入的 runner 可额外声明
  Columnar / BinaryPacked / ResourceBacked。非 Row encoder 是后续 host/core 优化点，
  不改变 TaskPool、TaskLease 或 runner ABI。
- `RunnerDescriptor.batch.mode` 必须声明 runner 是 `native_batch` 还是
  `scalar_adapter`；兼容旧 lock 的 `batch` 值只表示 legacy native batch 声明。未声明
  或使用默认 capability 时按 scalar adapter 处理，`max_entry_concurrency = 1`、
  `preserve_order = true`、`side_effect = unknown`、`entry_cancel = false`，
  adapter 内部串行执行 entry。
- `RunnerDescriptor.batch` 声明 preferred / max batch entries、max inflight batches、
  max entry concurrency、partial failure、preserve order、scalar thread safety /
  reentrancy 和 side-effect 能力；Core / host 仍会按 scheduler budget 和资源冲突
  对实际 batch 入场做更严格限制。当前单实例模型要求 `max_inflight_batches = 1`；该字段
  不限制同一 batch 的 entry 数或 entry-level parallelism。其他值必须在 load-plan /
  registry materialize 阶段结构化失败。
- `RunnerDescriptor.resources` 和 `RunnerDescriptor.ordering` 只声明 runner 能力边界；
  资源计划仍由 Core dispatch 前生成，排序事实仍来自 TaskPool 和 batch entry。

Core 不暴露 Rust `Future`、Promise、Coroutine、join/select、TaskGroup、WaitSet 或通用
executor。
Python runner kit 的 async adapter 只接受 Mutsuki task awaitable；普通 `asyncio`
Future 或其他语言 awaitable 必须结构化失败，不能被伪装成 Core 调度语义。

issue #5 的 SDK async/await 预期是语法层收敛，不是 Core 能力扩张：

- child task 必须继承 parent task 的 `trace_id` / `correlation_id`；`parent_task_id`
  由 `TaskAwait` 携带用于 trace/debug，不形成 Core `TaskGroup` 生命周期语义。
- `await` 只注册一个 child wait link，使当前 task Waiting / Suspended 并释放本 step
  的 `TaskLease`；同一 task continuation 不允许并发恢复。
- self-targeted await、非 Mutsuki awaitable 或 SDK 接管语言 event loop 都必须结构化失败，
  不得用 reentrant 执行、本地直调或伪装支持绕过 TaskPool。

`CancelPolicy::Cascade` 是当前实现语义；`Detach` / `Shield` 可以由 SDK 写入
`TaskHandle` / `TaskAwait` descriptor 以保持 wire shape 前向兼容，但 Core 暂不实现
完整生命周期语义。等待中的 parent task 若携带非 Cascade await 并被取消，Core 必须
结构化失败，而不是假装完成 Detach / Shield。

默认 HostRuntime 的 running cancel 分两段：public command 只暴露 `CancelTask(TaskId)`；
host 内部先让 Core task 进入 cancelled，再记录 pending runner cancel。HostRuntime 可由
`RunnerLimits.deadline_ticks` 为本次 runner invocation 生成 `deadline_tick`，并在后续
actor tick 中把超期 running task 标记为 cancelled。Host 可在不改变 wire shape 的前提下
叠加 wall-clock deadline、取消宽限和 worker health timeout；这些字段只存在于 host
config / runner limits。超时的 native worker 会被隔离并进入 drain，原线程退出前不得补
replacement；达到隔离上限时 Host degraded / 拒绝新 dispatch。若隔离后的 native runner
迟到返回，host 只投递 `Runner.cancel` / `Runner.dispose`，不提交旧结果，也不把旧 runner
放回 Core。process runner 可通过 host-local termination handle 强制终止子进程并在阻塞调用
返回后重建；该 handle 与恢复方法不是 wire contract，不提供 Rust 线程级强制终止。

`next_required_step` 是 Rust Host 的非 wire 调度提示：它只汇总 TaskPool 增量索引中未来的
ready/wake/lease step，不改变 Task、Runner 或 JSONL ABI。event-driven Host 将该 step 与
running invocation 的 tick/wall-clock supervision deadline 合并为一次性 timer；mailbox 事件
立即重新调度。timer 到期可以直接把 `current_step` 推进到目标 step，但仍必须执行完整的
lease reclaim、wake、expectation、scheduler budget 和 dispatch 校验。

HostRuntime completion subscription 同样是非 wire 控制面提示。它只发布单调 terminal
revision，允许慢消费者合并通知；`TaskStatesBatch(Vec<TaskHandle>)` 才从 actor-owned
TaskPool 读取权威 status/outcome。subscription 关闭或 actor 退出必须解除阻塞，且该机制
不表示 Core broadcast completion、TaskGroup 或第二份结果存储。

Core 的公开 task facade 以 `TaskHandle` descriptor 为入口。字符串 task id 只允许作为
TaskPool、cascade cancel、host actor bookkeeping 等内部事实键使用，不再作为公开
status / result / outcome / events / cancel / wake 入口。`TaskHandle` 不代表语言级
future、真实执行句柄或长期持有的 runtime object。

## 4. TaskPool

TaskPool claim 必须满足：

- task 是 ready。
- `ready_at_step` 未设置或已到达。
- runner 接受 task protocol id。
- runner hint 若存在必须匹配。
- task `registry_generation` 与当前 registry generation 匹配。
- task `expected_versions` 与当前 StateStore version 匹配；不匹配的 ready task 在
  dispatch 前结构化失败为 `state.conflict`，不得进入 runner。
- Effectful runner 只能 claim `effect.*` task。
- Committer runner 只能 claim `core.*` task。
- `effect.*` 只能由 Effectful runner claim。
- `core.*` 只能由 Committer runner claim。

TaskPool 入池必须保持 `task_id` 唯一。重复 `task_id` 必须结构化失败为
`task.duplicate`，不得覆盖已有 Ready / Running / Waiting / terminal record。
ResultRouter 派生 task 时也必须遵守同一规则，失败前不得修改既有 task record。
ResultRouter 必须在记录 value/resource lineage 或入队任何输出 task 前，整体预检本次
runner result 将派生/返回的全部 task id；若与 TaskPool 已有 record 或本次输出内部重复，
必须以 `task.duplicate` 失败，且不得留下部分派生 task 或 lineage 事件。

默认 TaskPool 在 runtime 生命周期内保留全部 terminal TaskRecord，因此 TaskHandle outcome
持续可查且 task id 永久防重。Core/Host 可以通过非 wire 的 `TaskHistoryRetention` 显式选择
有界历史：超过 `max_terminal_records` 的最旧 terminal record 连同 payload byte cache 一起
淘汰，仍被 wait link 引用的 child 暂缓淘汰；淘汰 id 进入容量为
`max_evicted_task_ids` 的防重窗口。旧 handle 在 record 淘汰后返回 `task.not_found`，防重窗口
过期后同一字符串 task id 可重新入池。该策略不能伪装成持久归档或永久幂等存储；需要长期
outcome/idempotency 的 Host 必须使用外部持久层或保持默认无限保留。

TaskRecord 仍是唯一权威记录；TaskPool 同步维护可从主记录完整重建的增量调度索引：

- ready task 使用 `ready_at_step asc -> priority desc -> created_sequence asc -> task_id asc`
  的稳定有序 key，并以 protocol、runner hint 和 owner runner 的复合 selector 索引。
- Waiting / Blocked 的 `ready_at_step`、Running lease expiry 和带 `expected_versions` 的 Ready
  task 分别进入到期桶或专用集合；tick 只处理到期桶和声明 expectation 的 task。
- Running / Waiting task 按 runner 维护集合，runner load 的 inflight 计数不得扫描全部 TaskRecord。
- enqueue、claim、wait、wake、block、cancel、retry、terminal、reload rebind 和 Abort 必须在同一
  TaskPool 变更内同步主记录与索引；registry generation 批量迁移后显式从主记录重建索引。

Task enqueue 时缓存 payload 的 compact JSON 编码字节数。`DispatchBudget.max_bytes` 对所选
payload 编码字节数之和执行硬限制；payload 在 TaskPool 中不可变，因此缓存值与相同
`serde_json` ABI payload 编码的允许偏差为 0。该预算不包含 WorkBatch / JSONL envelope、
lease、entry 或 resource plan 开销；具体 ABI/transport 仍必须在最终发送边界执行自己的完整
frame/message 硬限制，不能把 payload estimate 当作完整 frame 大小。

Task claim 成功后必须生成 `TaskLease`，Running 状态必须能追溯 runner、executor 和
lease id。当前第一段 executor supervision 中，默认 TaskLease 有效期为一个 tick：
`expires_at_step = acquired_at_step + 1`，当 `current_step >= expires_at_step` 时视为过期。
完成、失败、取消、等待或阻塞当前 task 时，Core 必须用 active TaskLease fencing 后才
能提交状态，并释放该 task lease。

每个 task record 维护本地单调递增的 attempt generation，并将其编码进既有
`TaskLease.lease_id`。因此同一 step 内发生 cancel/retry 后也不能复用旧 attempt token。
completion 必须原子匹配 Running 状态、runner、executor、registry generation 和完整
active lease；retry、reload rebind、deadline/cancel、lease expiry 或 runtime Abort 后的
迟到 completion 统一以 `task.claim_conflict` 拒绝。详细停止与观察契约见
[local-task-lifecycle.md](local-task-lifecycle.md)。

过期 TaskLease 不表示 Task terminal expiry。Core 在新一轮 claim 前回收过期 Running
task：将其恢复为 Ready，清空 claimed runner / executor / lease id，让它可被重新
claim。旧 executor 随后返回的结果必须以结构化 `task.claim_conflict` 失败，且不得
修改 task 状态、StateStore、EventLog 或派生新 task。

`RunnerStatus::Continue` 只表示当前 step 未完成，不续租、不长期占用 executor。若
runner 未在本 tick 内提交 terminal / waiting / blocked 状态，lease 到期后由 Core
回收为 Ready 后重试。`Continue` 不得携带 deltas/events/tasks/effects/values/resources；
否则必须在写入任何输出前结构化失败，避免 lease 回收重试造成重复派生输出。

当 runner 返回 `RunnerStatus::Waiting` 且携带 `TaskAwait`：

- Core 保存当前 task `continuation_ref`。
- Core 注册 child task terminal 状态到 parent task wake 的 wait link。
- `TaskAwait.child` 必须指向已存在或本次 runner result 显式返回的未终止 child task；
  child handle 的 protocol、target binding、trace/correlation 必须与 child task descriptor
  一致，且 child task 必须继承 parent task 的 trace/correlation。
- 当前 task 释放 `TaskLease`，runner 不因 await 被长期占用。
- 当前 task 保留 `owner_runner`，wake 后只能由原 runner reclaim continuation。
- Waiting task 计入 runner waiting / inflight 负载，调度器不能只看 running_count。
- child task completed / failed / cancelled / expired / dead_letter 后，parent task 被唤醒为 ready/runnable。
- `WakeCondition::Timer` / `RetryAfter` 写入 parent task `ready_at_step`；Core 在每轮
  dispatch 前将到期 Waiting / Blocked task 唤醒为 Ready，并移除该 parent 的 wait link，
  防止 child 后续 terminal 状态重复恢复同一 continuation。

排序：

```text
ready_at_step asc
priority desc
created_sequence asc
task_id asc
```

Scheduler v1 不是公开 wire contract，也不进入 `PluginManifest` / `RuntimeLoadPlan`。
当前只允许 host 级 `SchedulerPolicy` 返回每个 runner 本轮 dispatch budget：

- 输入来自 Core / host 只读 snapshot：runner descriptor、RunnerLoad、runner limits、
  HostCapacity、worker pool slots、hard capacity、current_step 和 registry_generation。
- 输出包含 scheduler id、reason、requested dispatch limit 和
  `DispatchBudget { max_entries, max_batches, max_bytes, lane_budget }`。
- host 必须将 requested limit 和 budget max_entries clamp 到 hard capacity。
- 当前单实例模型还必须将 `DispatchBudget.max_batches` clamp 到 `0..=1`；非零 entry
  budget 也不能声明同一 runner 存在第二个 active batch。
- Core 继续执行 runner acceptance、TaskPool 排序、WorkBatch 构造、TaskLease 创建和状态提交。
- Core 在 claim 时必须按 `DispatchBudget` 筛选 ready task；lane budget 只限制本 batch
  各 `DispatchLane` 可进入的 entry 数，不能改变 TaskPool 的基础排序。
- `HostCapacity` 必须暴露 running / queued batch 与 entry 数、saturation、
  preferred batch size、max running batches、max entry concurrency 和 max inflight bytes，
  供 host scheduler 只读决策。当前 `max running batches` 固定为 `1`，已有 active batch
  时 hard capacity 必须为 `0`。
- scheduler 不能执行 task、不能创建子 task、不能完成 task、不能修改 TaskPool、
  不能访问真实资源本体。

后续 scheduler provider / plugin 化必须先扩展 plans / contracts，并保持上述权限边界。

## 5. ResultRouter

Pure runner 不直接提交状态或执行副作用：

- `status = completed / failed / cancelled / waiting / blocked` 只改变当前 task 的状态。
- `task_await` 只注册一个 child wait link，不表示 TaskGroup。
  `task_await` 只能与 `status = waiting` 同时出现；其他状态携带 `task_await` 必须
  在派生 child task 前结构化失败。
- `deltas` 生成 `core.commit` task。
- `events` 生成 `core.event.append` task。
- `effects` 生成 `effect.*` task。
  `EffectRequest.preconditions` 必须映射为派生 effect task 的 `expected_versions`，
  由 Core 在 dispatch 前统一校验。
- `values` / `resources` 记录 value/resource lineage。
- `tasks` 直接进入 TaskPool。复杂编排必须由插件 runner 显式返回这些 task；Core 不根据
  protocol 或 handler binding 自动 fan-out。

`deltas` 和 `effects` 是 Pure runner 的派生输出面；Effectful / Committer runner 返回
这些字段时必须结构化失败为 `runner.purity_violation`，不得静默丢弃，也不得先派生
`core.commit` 或二级 `effect.*` task。

`RunnerStatus::Continue` 只表示本 step 尚未产出可提交事实；携带任何输出字段必须
结构化失败为 `task.claim_conflict`，不得写入 TaskPool、EventLog 或 lineage。

Committer runner 是 StateStore/EventLog 的唯一提交入口。`Waiting` / `Blocked` 只描述
当前 task，不表示 TaskGroup、WaitSet、broadcast completion 或 workflow stage。

## 6. Resource / Value

Task payload 可包含：

- scalar immediate。
- 小型不可变 inline value。
- `ValueRef`。
- `ResourceRef`。
- `ResourceCellRef`。
- `ResourceLease`。
- `StateRef`。

`ResourceRef` 必须包含兼容 `ref_id` 和结构化 `ResourceId`：

```text
ResourceId {
  kind_id,
  slot_id,
  generation,
  version
}
```

`kind_id` 选择 typed store，`slot_id` 是 store 内部槽位，`generation` 防止槽位复用命中旧
handle，`version` 用于 snapshot、冲突检测和回放。`ResourceRef.generation/version` 与
`ResourceRef.resource_id.generation/version` 必须一致。

资源语义分类：

- `FrozenValue`：完全不可变值或 blob。
- `VersionedSnapshot`：可 stale 使用的不可变快照。
- `ReadOnlyFact`：普通插件只读、Host 可刷新事实。
- `CowVersionedState`：通过 patch/transaction 提交的可变版本化状态。
- `CapabilityResource`：能力资源，只能通过 command plan 调用。
- `StreamResource`：流式、背压、可取消资源。
- `TransactionResource`：事务或批量变更状态。

默认一致性规则：

- 共享资源 readonly/sealed。
- 修改生成新 ref。
- 状态修改走 `StateDelta + expected_version`。
- 原地写必须 ExclusiveWriteLease。
- runner 可以持有短期 ResourceLease，但不能拥有 ResourceCell。
- lease 过期、generation mismatch、provider 崩溃必须结构化失败。
- 短期可变 lease 不允许默认跨 SDK await；await 前存在当前 task 持有的
  exclusive write lease 或 exclusive ResourceLease 时必须结构化失败。

资源操作计划规则：

- `ReadPlan` 构造不访问资源；`eval` / `collect` / `snapshot` / `open_stream` 才执行读。
- `WritePlan` 构造不修改资源；`commit` 时检查 `base_version` / generation / lease 后写入。
- Provider 执行 `WritePlan` commit、`CommandPlan`、`CommandBatch` 或 `SagaPlan` 后，必须通过
  `PlanReceipt.descriptor_updates` 返回需要进入 ResourceManager registry 的新
  `ResourceRef` descriptor；包括原 ref 的新 generation/version、copy-on-write 产生的新 ref、
  command 创建的 capability/snapshot/transaction ref。Host 只把这些 descriptor 回写
  ResourceManager，bytes 数据面不得回流 Core。
- `ExportPlan` v1 的标准 memory provider 支持 `target = "inline_utf8"`：执行时读取
  provider 资源 bytes，经 UTF-8 解码后放入 `PlanReceipt.output`；未知 target、非 UTF-8
  bytes 或 stale descriptor 必须结构化失败。
- `CommandPlan` v1 的标准 memory provider 支持 `CapabilityResource` 上的
  `operation = "query"`，返回 deterministic command receipt；不执行真实外部副作用，
  不承诺 provider RPC、幂等去重或事务保证。
- `TransactionPlan` / `CommandBatch` / `SagaPlan` 是 experimental provider/workflow
  descriptor。CoreRuntime 不解释、调度或执行事务、批处理、补偿或 workflow 语义。
  Provider 或 workflow plugin 可以在自身边界实现这些计划，并通过 `PlanReceipt`
  只回写 descriptor 更新。
- 若 provider 支持 experimental `CommandBatch` / `SagaPlan`，`CommandBatch.rollback_guarantee = true`
  在 v1 中应结构化失败；`SagaPlan` 可按顺序执行 steps，step 失败后按反序尝试
  compensations，并以 `resource.saga_failed` 返回原始 cause。
- 公共插件 API 不暴露 `Arc<T>`、`&T`、`&mut T`、`downcast`、`with_native_*` 或闭包式 lazy op。

Resource Provider 热替换规则：

- `ResourceTypeDescriptor.kind_id`、`semantic`、`schema`、`provider_id` 和 `operations` 是
  provider surface fingerprint 的一部分。
- `ResourceProviderCompatibility.required_operations` 是旧 live resource 在新 provider
  上继续可用的最小 operation 集；减少这些 operation 必须视为 breaking。
- `preserves_resource_type_id = true` 表示新 provider 继续识别旧 `ResourceId.kind_id`；
  否则存在 live resource 时不能热替换。
- `accepts_older_generations = true` 表示新 provider 能识别旧 generation 的 descriptor；
  否则必须 zero occupancy、迁移或重启。
- `lease_drain_required = true` 表示存在 active lease / stream / mutable transaction 时必须
  drain、cancel、freeze、migrate 或阻断，不能原地切换。
- `ResourceProviderReloadPolicy` 只描述 provider 边界；Resource Registry、ResourceId 分配、
  lease 基础规则和权限入口仍属于 Core / ResourceManager。

## 7. Plugin Loading

Core 只消费 `RuntimeLoadPlan`：

- 校验 runner descriptor 不超出 manifest/load-plan 授权。
- 构建 registry。
- freeze registry。
- 记录 registry generation。

`PluginManifest` 描述插件能力与 artifact；`RuntimeProfile.plugin_deployments` 描述本次
运行选择 builtin / ABI / WASM / process / Python 中哪一种部署形态。resolver 必须把每个
enabled plugin 的部署形态写入 `RuntimeLoadPlan.plugin_deployments`，并校验部署形态与
artifact 类型兼容。部署形态属于 host 执行面约束，不得进入插件业务代码分支。

`PluginManifest::business_surface` 是跨 deployment 的业务等价判断权威。它忽略 artifact、
lifecycle、Host extension、plugin backend、codec 和 bridge，只比较领域无关的业务契约。
ABI bridge v2 必须先执行 `plugin.initialize({ config })`，且 guest 返回的 manifest 必须与
安装清单一致；未初始化连接不得调用 Runner 或 Resource 方法。

`RuntimeProfile.mode` 描述发行形态：

- `full_dev`：开发模式，resolver 保留 manifest 声明的系统扩展 surface，便于调试和覆盖测试。
- `extensible_runtime`：允许外部插件，resolver 保守保留外部 backend / bridge / codec 等 host 扩展。
- `builtin_only`：只允许 builtin 插件集合，resolver 可按当前 enabled plugins 与 deployment 裁剪未使用注册路径。
- `locked_builtin`：插件集合固定，resolver 必须生成静态 active capability graph，host 只声明当前 graph 中的 backend / bridge / codec / workflow / scheduler policy。

Host backend 裁剪是 Level 2 boot gate：不注册、不声明未激活的实现 surface；不改变
Core 抽象和 wire shape。`RuntimeLoadPlan.capability_graph` 必须记录 provided /
required / active capabilities、active capability provider selection、permission audit，
以及 active resource providers、host backends、plugin backends、codecs、bridges、
scheduler policies 和 workflows。Host capability 必须从这个 active graph 或实际
registry 生成，不能在裁剪后继续宣称已裁剪的 bridge、codec 或 resource strategy 可用。
RuntimeBootstrapper / host boot 必须在 CoreRuntime 启动前校验 active plugin backend
引用的 bridge deployment 与 codec 支持关系，并拒绝不匹配 active scheduler descriptor 的
host scheduler policy 实例。

`PluginManifest.requires` 兼容既有 capability 字符串，并可用
`<capability>@<version-constraint>` 表达 resolver 级版本约束。resolver 只校验 load plan
内已声明 provider descriptor 的版本，不负责下载、安装或跨版本依赖解算。插件声明的
effect / resource permission 必须能映射到 active effect、resource provider、resource type
或 resource operation；无法映射时 resolver 必须结构化失败，不能生成看似授权的 load plan。

Builtin 插件必须仍通过 host 注册 runner/provider 能力，不能因为静态编译进 Host 就进入
Core 内建逻辑。ABI / WASM / process / Python 插件必须通过对应 host bridge 注册相同的
runner/task/resource 协议面；插件业务代码只能依赖统一插件 API 和 ResourceRef / plan /
Task descriptor，不得获得 builtin-only native 引用。

插件运行中不得动态注册未授权 capability。如需变更，必须生成新的 load plan 和
registry generation。

Host 执行面必须把部署形态限制在后端实现中：

- builtin / static 插件通过 `LocalTaskClient` 和 provider-backed
  `LocalResourceClient` 使用本地快速路径；resource 创建和 plan 数据面必须经
  `ResourceProviderGateway`，不能回退到 core-managed descriptor/store。
- builtin / static 插件若在 load plan 中声明 active resource provider，必须随
  `LoadedPlugin` 注册同 id 的 `ResourceProviderGateway` 实例；Host 启动时按
  `RuntimeLoadPlan.capability_graph.active_resource_providers` 自动注入。active
  provider 缺少实例必须结构化失败，不能静默退回 core 兼容 store。
- ABI 插件通过 `AbiTaskClient`、`AbiResourceClient` 将同一 `Task` / resource plan
  wire shape 编码到 bridge。
- ABI 动态库入口只承载版本化 connection 与 UTF-8 JSONL bytes request/response；guest-side
  shim 必须把这些 bytes 映射回上述既有 batch-first runner/task/resource 方法，禁止再定义
  一套按语言对象、Rust trait 或 native pointer 展开的插件 ABI。
- 两条路径不得向插件业务代码暴露 `Arc<T>`、`&T`、`&mut T`、`downcast` 或
  `with_native_*` 之类 builtin-only 能力。
- `ReadPlan` 的 collect / snapshot / stream open、`WritePlan` 的 commit，以及
  export / command / batch / saga plan 都必须在后端边界执行；构造 plan 本身不得读写资源。

系统扩展插件边界：

- `HostExtensionDescriptor` 只能声明 Host 内部 service/backend，例如 plugin backend、bridge、
  codec、trace sink、resource backend、permission policy 或 scheduler policy；Host shell
  本身不是插件。
- `PluginBackendDescriptor` 把 builtin / ABI / WASM / process / Python 部署形态映射到统一
  `TaskClient` / `ResourcePlanClient` 协议；插件业务代码不能根据部署形态获得 native-only 引用。
- `CodecDescriptor.connection_scoped = true` 表示 codec 在连接握手时确定，连接生命周期内不能切换。
- `BridgeDescriptor.drain_policy` 必须表达旧连接 drain / cancel / restart 需求；活跃 ABI call /
  IPC call 中途不得切换 bridge 或 codec。
- `SchedulerPolicyDescriptor.decision_scope` 当前只能是 host dispatch budget 类策略，不能选择
  具体 task、执行 task、修改 TaskPool 或读取真实资源本体。
- `WorkflowDescriptor.state_resource_kind` 必须指向外置 workflow instance resource；workflow
  实例状态不能藏在插件内存里作为热替换事实源。

## 8. Hot Reload

Contract surface 兼容性：

- Identical：可热重载。
- Additive：可热重载。
- Deprecated：可热重载，但禁止新增占用。
- Removed：必须 zero occupancy。
- Breaking：必须迁移、drain 或 restart。

Core 热重载必须使用新 registry / plugin generation，不原地替换 runner。切换时：

- ready task 可以 rebind 到新 registry generation。
- clean / local dirty running invocation 应通过原 runner 的 cancel 管理面回到
  ready，再交给新 generation 重试。
- polluted / unknown dirty running invocation 必须保留旧 generation drain，或由上层
  提供明确 compensation；不得强行 dispose。
- removed surface 的 zero occupancy 判定必须来自 TaskPool、ResourceManager 等当前
  事实源，而不是手动缓存。
- effect occupancy 来自 ready/running `effect.*` task；stream occupancy 来自
  `ResourceAccess::Stream` 资源和显式 `SurfaceOccupancyHandle`；subscription/timer
  occupancy 来自显式 `SurfaceOccupancyHandle`。
- deprecated surface 禁止新增派生占用：task enqueue 必须检查 task protocol、effect kind、
  runner hint 和 required surfaces；stream/subscription/timer 注册入口必须检查目标
  surface。

`HandlerBinding` 是查询索引，不是 Core 内置分发规则。已经由插件编排过的输入不因新增
binding 自动重新 fan-out；补跑必须显式生成 migration/backfill task。

## 9. 标准错误码

| code | 场景 |
|---|---|
| `task.not_found` | task 不存在 |
| `task.duplicate` | 入池或派生 task 使用了已存在的 task id |
| `task.claim_conflict` | task completion/failure 与 claim runner 不匹配 |
| `task.expired` | runtime / host 将 task 结构化终止为 expired |
| `task.dead_letter` | runtime / host 将 task 结构化终止为 dead_letter |
| `task.unsupported` | core kernel 收到已授权但不支持的 kernel task |
| `task.cancel_policy_unsupported` | 等待中的 parent task 使用当前 Core 尚未实现的取消策略 |
| `task.self_call_blocked` | targeted SDK await 会回到当前 runner 且 self-call policy 禁止 |
| `runner.not_found` | runner 不存在 |
| `runner.purity_violation` | runner purity 与 task/result 不兼容 |
| `runner.awaitable_unsupported` | Python runner-side async adapter 收到非 Mutsuki awaitable |
| `runtime.not_accepting` | runtime 正在 Drain，不再接受新的外部 task |
| `runtime.aborted` | runtime 已 Abort，拒绝 submit、tick、claim 和 completion |
| `portable.schema_unsupported` | portable task envelope schema id/version 不受支持 |
| `checkpoint.incompatible` | checkpoint 与 task schema、实现 generation 或 input identity 不兼容 |
| `execution.no_variant` | 本地能力无法满足 variant 与显式质量/no-placement 策略 |
| `registry.frozen` | freeze 后动态注册 |
| `registry.unauthorized` | descriptor 超出 load plan 授权 |
| `registry.generation_mismatch` | task/descriptor registry generation 不匹配 |
| `state.conflict` | expected_version 不匹配 |
| `resource.not_found` | value/resource ref 不存在 |
| `resource.lease_cross_await` | task await 前仍持有短期可变资源租约 |
| `resource.unsupported` | resource provider 不支持该 operation、target、resource semantic 或 experimental guarantee |
| `resource.lease_expired` | lease 过期 |
| `resource.generation_mismatch` | generation 不匹配或 stale lease |
| `plugin.reload_blocked` | hot reload 被 breaking/occupancy 阻断 |
| `capability.exhausted` | lease/capability 容量耗尽 |
| `runtime.host_failed` | host/runner 无法归类的失败 |

## 10. Crate 对应

- `crates/mutsuki-runtime-contracts`：本文件协议对象。
- `crates/mutsuki-runtime-core`：CoreRuntime、TaskPool、RunnerRegistry、ResourceManager。
- `crates/mutsuki-runtime-host`：runtime bootstrapper、load-plan resolver、JSONL runner client。
- `crates/mutsuki-runtime-sdk`：Rust SDK async/task/resource helper，以及 host/plugin
  扩展基础 trait；本次没有新增 wire protocol object。
- 外部 `MutsukiPythonRunnerKit`：Python mirror、runner backend、stdio bridge、resource manager。
