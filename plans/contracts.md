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
| `RunnerDescriptor` | runner_id、plugin_id、generation、accepted_protocol_ids、purity、schema、metadata |
| `RunnerPurity` | Pure、Committer、Effectful |
| `RunnerResult` | task_id、deltas、events、tasks、effects、values、resources、status |
| `StateDelta` | target_ref、expected_version、patch、conflict_policy |
| `EffectRequest` | effect_id、kind、payload、preconditions、idempotency_key |
| `ValueRef` | 小型结构化共享值 descriptor |
| `ResourceRef` | 大型资源 / mmap / blob / stream / provider-RPC descriptor |
| `ResourceCellRef` | 长期资源状态单元 descriptor，例如连接池、stream、cookie jar、rate limiter |
| `ResourceLease` | task step 临时使用 ResourceCell 的租约 |
| `LeaseToken` | ref_id、owner、mode、expires_at_step、generation |
| `RuntimeProfile` | 本次运行启用哪些插件、绑定哪些能力、是否允许热重载 |
| `PluginManifest` | 插件声明 runner、protocol、handler binding、resource schema/provider、effect、stream、subscription、timer、permission、lifecycle |
| `RuntimeLoadPlan` | resolver 生成的确定性加载计划和 registry generation |
| `ContractSurface` | runner/task/schema/resource/effect/stream/subscription/timer/lifecycle/permission 等热重载比较单元 |
| `SurfaceOccupancyHandle` | stream/subscription/timer 等 lifecycle 占用 descriptor |
| `RuntimeEvent` | sequence、kind、name、subject_id、attributes、error |
| `TraceSpan` | trace_id、span_id、parent_span_id、name、interval、attributes、status |

## 3. Task 与 Runner 接口

`Task.protocol_id` 是当前调度事实源。早期 task-kind 兼容字段不再属于当前 wire shape。

```text
Runner.step(ctx, tasks) -> Vec<RunnerResult>
Runner.cancel(invocation_id)
Runner.dispose()
```

`RunnerContext` 至少包含：

- `registry_generation`
- `current_step`
- `executor_id`
- `task_lease_id`

当前 `tasks` 仍保留 Vec wire shape 以兼容 host/JSONL runner client，但 Core 每次只
lease 一个 Task 给一个 Executor 调用 Runner。

SDK 层可以把 task 原语包装成语言 awaitable：

```text
Rust SDK: ctx.call(protocol, input).await -> TaskOutcome
JS/Python SDK: future package 可包装同一 TaskHandle / TaskOutcome wire shape
```

Core 不暴露 Rust `Future`、Promise、Coroutine、join/select、TaskGroup、WaitSet 或通用
executor。

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
lease id。完成、失败、取消、等待或阻塞当前 task 时，Core 释放该 task lease。

当 runner 返回 `RunnerStatus::Waiting` 且携带 `TaskAwait`：

- Core 保存当前 task `continuation_ref`。
- Core 注册 child task terminal 状态到 parent task wake 的 wait link。
- 当前 task 释放 `TaskLease`，runner 不因 await 被长期占用。
- child task completed / failed / cancelled / expired / dead_letter 后，parent task 被唤醒为 ready/runnable。

排序：

```text
ready_at_step asc
priority desc
created_sequence asc
task_id asc
```

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

默认一致性规则：

- 共享资源 readonly/sealed。
- 修改生成新 ref。
- 状态修改走 `StateDelta + expected_version`。
- 原地写必须 ExclusiveWriteLease。
- runner 可以持有短期 ResourceLease，但不能拥有 ResourceCell。
- lease 过期、generation mismatch、provider 崩溃必须结构化失败。
- 短期可变 lease 不允许默认跨 SDK await；await 前存在当前 task 持有的
  exclusive write lease 或 exclusive ResourceLease 时必须结构化失败。

## 7. Plugin Loading

Core 只消费 `RuntimeLoadPlan`：

- 校验 runner descriptor 不超出 manifest/load-plan 授权。
- 构建 registry。
- freeze registry。
- 记录 registry generation。

插件运行中不得动态注册未授权 capability。如需变更，必须生成新的 load plan 和
registry generation。

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
| `runner.not_found` | runner 不存在 |
| `runner.purity_violation` | runner purity 与 task/result 不兼容 |
| `registry.frozen` | freeze 后动态注册 |
| `registry.unauthorized` | descriptor 超出 load plan 授权 |
| `registry.generation_mismatch` | task/descriptor registry generation 不匹配 |
| `state.conflict` | expected_version 不匹配 |
| `resource.not_found` | value/resource ref 不存在 |
| `resource.lease_expired` | lease 过期 |
| `resource.generation_mismatch` | generation 不匹配或 stale lease |
| `plugin.reload_blocked` | hot reload 被 breaking/occupancy 阻断 |
| `capability.exhausted` | lease/capability 容量耗尽 |
| `runtime.host_failed` | host/runner 无法归类的失败 |

## 10. Crate 对应

- `crates/mutsuki-runtime-contracts`：本文件协议对象。
- `crates/mutsuki-runtime-core`：CoreRuntime、TaskPool、RunnerRegistry、ResourceManager。
- `crates/mutsuki-runtime-host`：native runner host、load-plan resolver、JSONL runner client。
- `python/mutsuki-runtime-python`：Python mirror、runner host、stdio server、resource manager。
