# Mutsuki Runtime Contracts

根级 Rust contracts 是当前协议事实源。Python runner kit 必须镜像这些 wire shape。

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
| `Task` | 统一待处理控制消息，包含 protocol_id、priority、ready_at_step、payload、refs、target_binding_id、lease_id、expected_versions、registry_generation |
| `TaskLease` | 一次 task step 的执行租约，绑定 task、runner、executor、registry generation 和租约时间 |
| `TaskHandle` | SDK-facing task descriptor，包含 task id、protocol、target binding、取消策略和 trace/correlation |
| `TaskAwait` | 当前 task 等待一个 child task 的 continuation registration |
| `TaskOutcome` | SDK 读取的 terminal task 结果映射 |
| `CancelPolicy` | SDK await 取消策略，当前默认支持 Cascade，Detach / Shield 为协议预留 |
| `TaskStatus` | created、ready、running、waiting、blocked、completed、failed、cancelled、expired、dead_letter |
| `ProtocolDescriptor` | protocol_id、schema、codec、version、compatibility 等纯数据契约 |
| `HandlerBinding` | 插件对 protocol 的逻辑消费绑定，指向目标 protocol / runner hint / pool |
| `RunnerDescriptor` | runner_id、plugin_id、generation、accepted_protocol_ids、purity、execution_class、schema、metadata |
| `ExecutionClass` | host 执行池分类：Control、Orchestration、Io、Cpu、Blocking、Script |
| `RunnerPurity` | Pure、Committer、Effectful |
| `RunnerResult` | task_id、deltas、events、tasks、effects、values、resources、status |
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
| `SnapshotDescriptor` / `PatchDescriptor` / `PlanReceipt` | 版本化 snapshot、patch 与 plan commit receipt |
| `TransactionPlan` / `CommandBatch` / `SagaPlan` | 严格事务、无回滚批量命令和带补偿的 saga 计划 |
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
| `WorkflowDescriptor` | workflow 插件与外置实例状态资源的绑定 descriptor |
| `RuntimeLoadPlan` | resolver 生成的确定性加载计划和 registry generation |
| `ContractSurface` | runner/task/schema/resource/effect/stream/subscription/timer/lifecycle/permission 等热重载比较单元 |
| `SurfaceOccupancyHandle` | stream/subscription/timer 等 lifecycle 占用 descriptor |
| `RuntimeEvent` | sequence、kind、name、subject_id、attributes、error |
| `TraceSpan` | trace_id、span_id、parent_span_id、name、interval、attributes、status |

## 3. Task 与 Runner 接口

`Task.protocol_id` 是当前调度事实源。wire shape 不包含额外 task kind 兼容字段。

```text
Runner.step(ctx, tasks) -> Vec<RunnerResult>
Runner.cancel(invocation_id)
Runner.dispose()
```

`RunnerDescriptor.execution_class` 只描述 host 物理执行池选择，不是业务协议语义。
`Control` 仅用于 core kernel 控制任务；普通插件不得因为声明 Control 而在 core 控制面
执行。

`RunnerContext` 至少包含：

- `registry_generation`
- `current_step`
- `executor_id`
- `task_lease_id`
- `invocation_id`
- `cancel_token`
- `deadline_tick`
- `cancel_requested`

当前 `tasks` 仍保留 Vec wire shape 以兼容 host/JSONL runner client，但 Core 每次只
lease 一个 Task 给一个 Executor 调用 Runner。

SDK 层可以把 task 原语包装成语言 awaitable。当前仓库内 issue #5 的落点是 Rust
SDK 与 `python/mutsuki-runtime-python` runner kit；JS/TS SDK 只作为同一
`TaskHandle` / `TaskOutcome` wire shape 的后续外部 SDK 目标，不在当前 workspace
新增占位包。

```text
Rust SDK: ctx.call::<Protocol>(input).await -> TaskOutcome
Rust SDK raw: ctx.call_raw(protocol_id, payload).await -> TaskOutcome
Python runner kit: await ctx.call_raw(protocol_id, payload) -> TaskOutcome
JS/TS SDK: future package 可包装同一 TaskHandle / TaskOutcome wire shape
```

Rust SDK 的 derive / attribute 宏只生成同一 `ProtocolDescriptor`、`ResourceTypeDescriptor`、
`RunnerDescriptor`、`PluginManifest` 和 `AsyncRunnerAdapter` glue；它们不是新的 wire
protocol object，也不能引入 workflow、broadcast、本地直调或绕过 TaskPool 的执行语义。

Core 不暴露 Rust `Future`、Promise、Coroutine、join/select、TaskGroup、WaitSet 或通用
executor。
Python runner kit 的 async adapter 只接受 Mutsuki task awaitable；普通 `asyncio`
Future 或其他语言 awaitable 必须结构化失败，不能被伪装成 Core 调度语义。

issue #5 的 SDK async/await 预期是语法层收敛，不是 Core 能力扩张：

- child task 必须继承 parent task 的 `trace_id` / `correlation_id`；`parent_task_id`
  只用于 trace/debug，不形成 Core `TaskGroup` 生命周期语义。
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
config / runner limits。超时的 native worker 会被隔离并进入 drain，pool 可补充
replacement worker。若隔离后的 runner 迟到返回，host 只投递 `Runner.cancel` /
`Runner.dispose`，不提交旧结果，也不把旧 runner 放回 Core。单连接同步 JSONL step 的
独立 management channel 和进程级强制终止属于 host/sidecar 执行面，不进入 contracts。

Core 保留既有 string task id facade，同时提供以 `TaskHandle` descriptor 为入口的
status / result / outcome / events / cancel / wake facade。`TaskHandle` 不代表语言级
future、真实执行句柄或长期持有的 runtime object。

## 4. TaskPool

TaskPool claim 必须满足：

- task 是 ready。
- `ready_at_step` 未设置或已到达。
- runner 接受 task protocol id。
- runner hint 若存在必须匹配。
- task `registry_generation` 与当前 registry generation 匹配。
- Effectful runner 只能 claim `effect.*` task。
- Committer runner 只能 claim `core.*` task。
- `effect.*` 只能由 Effectful runner claim。
- `core.*` 只能由 Committer runner claim。

Task claim 成功后必须生成 `TaskLease`，Running 状态必须能追溯 runner、executor 和
lease id。当前第一段 executor supervision 中，默认 TaskLease 有效期为一个 tick：
`expires_at_step = acquired_at_step + 1`，当 `current_step >= expires_at_step` 时视为过期。
完成、失败、取消、等待或阻塞当前 task 时，Core 必须用 active TaskLease fencing 后才
能提交状态，并释放该 task lease。

过期 TaskLease 不表示 Task terminal expiry。Core 在新一轮 claim 前回收过期 Running
task：将其恢复为 Ready，清空 claimed runner / executor / lease id，让它可被重新
claim。旧 executor 随后返回的结果必须以结构化 `task.claim_conflict` 失败，且不得
修改 task 状态、StateStore、EventLog 或派生新 task。

`RunnerStatus::Continue` 只表示当前 step 未完成，不续租、不长期占用 executor。若
runner 未在本 tick 内提交 terminal / waiting / blocked 状态，lease 到期后由 Core
回收为 Ready 后重试。

当 runner 返回 `RunnerStatus::Waiting` 且携带 `TaskAwait`：

- Core 保存当前 task `continuation_ref`。
- Core 注册 child task terminal 状态到 parent task wake 的 wait link。
- 当前 task 释放 `TaskLease`，runner 不因 await 被长期占用。
- 当前 task 保留 `owner_runner`，wake 后只能由原 runner reclaim continuation。
- Waiting task 计入 runner waiting / inflight 负载，调度器不能只看 running_count。
- child task completed / failed / cancelled / expired / dead_letter 后，parent task 被唤醒为 ready/runnable。

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
  worker pool slots、hard capacity、current_step 和 registry_generation。
- 输出只包含 scheduler id、reason 和 requested dispatch limit。
- host 必须将 requested limit clamp 到 hard capacity。
- Core 继续执行 runner acceptance、TaskPool 排序、TaskLease 创建和状态提交。
- scheduler 不能执行 task、不能创建子 task、不能完成 task、不能修改 TaskPool、
  不能访问真实资源本体。

后续 scheduler provider / plugin 化必须先扩展 plans / contracts，并保持上述权限边界。

## 5. ResultRouter

Pure runner 不直接提交状态或执行副作用：

- `status = completed / failed / cancelled / waiting / blocked` 只改变当前 task 的状态。
- `task_await` 只注册一个 child wait link，不表示 TaskGroup。
- `deltas` 生成 `core.commit` task。
- `events` 生成 `core.event.append` task。
- `effects` 生成 `effect.*` task。
- `values` / `resources` 记录 value/resource lineage。
- `tasks` 直接进入 TaskPool。复杂编排必须由插件 runner 显式返回这些 task；Core 不根据
  protocol 或 handler binding 自动 fan-out。

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
- `ExportPlan` v1 只支持 `target = "inline_utf8"`：执行时读取资源 bytes，经 UTF-8
  解码后放入 `PlanReceipt.output`；未知 target、非 UTF-8 bytes 或 stale descriptor
  必须结构化失败。
- `CommandPlan` v1 只支持 `CapabilityResource` 上的 `operation = "query"`，返回
  deterministic command receipt；不执行真实外部副作用，不承诺 provider RPC、幂等去重
  或事务保证。
- `TransactionPlan` 要求 strict all-or-nothing；`CommandBatch` 只表示批量发送，不保证回滚；
  `SagaPlan` 表示多个不可原子回滚步骤和可选补偿。
- `CommandBatch.rollback_guarantee = true` 在 v1 中结构化失败；`SagaPlan` 按顺序执行
  steps，step 失败后按反序尝试 compensations，并以 `resource.saga_failed` 返回原始
  cause。
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

`RuntimeProfile.mode` 描述发行形态：

- `full_dev`：开发模式，resolver 保留 manifest 声明的系统扩展 surface，便于调试和覆盖测试。
- `extensible_runtime`：允许外部插件，resolver 保守保留外部 backend / bridge / codec 等 host 扩展。
- `builtin_only`：只允许 builtin 插件集合，resolver 可按当前 enabled plugins 与 deployment 裁剪未使用注册路径。
- `locked_builtin`：插件集合固定，resolver 必须生成静态 active capability graph，host 只声明当前 graph 中的 backend / bridge / codec / workflow / scheduler policy。

第一阶段裁剪是 Level 1：不注册、不声明未激活的实现 surface；不改变 Core 抽象和 wire
shape。`RuntimeLoadPlan.capability_graph` 必须记录 provided / required / active
capabilities、active capability provider selection、permission audit，以及 active
resource providers、host backends、plugin backends、codecs、bridges、scheduler
policies 和 workflows。Host capability 应从这个 active graph 或实际 registry 生成，
不能在裁剪后继续宣称已裁剪的 bridge、codec 或 resource strategy 可用。

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

- builtin / static 插件通过 `LocalTaskClient`、`LocalResourceClient` 直连本地
  `CoreRuntime` / `ResourceManager` 快速路径。
- ABI 插件通过 `AbiTaskClient`、`AbiResourceClient` 将同一 `Task` / resource plan
  wire shape 编码到 bridge。
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
| `task.claim_conflict` | task completion/failure 与 claim runner 不匹配 |
| `task.cancel_policy_unsupported` | 等待中的 parent task 使用当前 Core 尚未实现的取消策略 |
| `task.self_call_blocked` | targeted SDK await 会回到当前 runner 且 self-call policy 禁止 |
| `runner.not_found` | runner 不存在 |
| `runner.purity_violation` | runner purity 与 task/result 不兼容 |
| `runner.awaitable_unsupported` | Python runner-side async adapter 收到非 Mutsuki awaitable |
| `registry.frozen` | freeze 后动态注册 |
| `registry.unauthorized` | descriptor 超出 load plan 授权 |
| `registry.generation_mismatch` | task/descriptor registry generation 不匹配 |
| `state.conflict` | expected_version 不匹配 |
| `resource.not_found` | value/resource ref 不存在 |
| `resource.lease_cross_await` | task await 前仍持有短期可变资源租约 |
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
- `python/mutsuki-runtime-python`：Python mirror、runner host、stdio server、resource manager。
