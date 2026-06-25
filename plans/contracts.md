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
| `Task` | 统一待处理控制消息，包含 kind、priority、ready_at_step、payload、refs、expected_versions、registry_generation |
| `TaskStatus` | pending、running、completed、failed、cancelled |
| `TaskDemand` | raw input/event 到目标 task kind 的 fan-out 规则 |
| `RunnerDescriptor` | runner_id、plugin_id、generation、accepted_task_kinds、purity、schema、metadata |
| `RunnerPurity` | Pure、Committer、Effectful |
| `RunnerResult` | task_id、deltas、events、tasks、effects、values、resources、status |
| `StateDelta` | target_ref、expected_version、patch、conflict_policy |
| `EffectRequest` | effect_id、kind、payload、preconditions、idempotency_key |
| `ValueRef` | 小型结构化共享值 descriptor |
| `ResourceRef` | 大型资源 / mmap / blob / stream / provider-RPC descriptor |
| `LeaseToken` | ref_id、owner、mode、expires_at_step、generation |
| `RuntimeProfile` | 本次运行启用哪些插件、绑定哪些能力、是否允许热重载 |
| `PluginManifest` | 插件声明 runner、task demand、resource schema/provider、effect、stream、subscription、timer、permission、lifecycle |
| `RuntimeLoadPlan` | resolver 生成的确定性加载计划和 registry generation |
| `ContractSurface` | runner/task/schema/resource/effect/stream/subscription/timer/lifecycle/permission 等热重载比较单元 |
| `SurfaceOccupancyHandle` | stream/subscription/timer 等 lifecycle 占用 descriptor |
| `RuntimeEvent` | sequence、kind、name、subject_id、attributes、error |
| `TraceSpan` | trace_id、span_id、parent_span_id、name、interval、attributes、status |

## 3. Runner 接口

```text
Runner.step(ctx, tasks) -> Vec<RunnerResult>
Runner.cancel(invocation_id)
Runner.dispose()
```

`RunnerContext` 至少包含：

- `registry_generation`
- `current_step`

## 4. TaskPool

TaskPool claim 必须满足：

- task 是 pending。
- `ready_at_step` 未设置或已到达。
- runner 接受 task kind。
- runner hint 若存在必须匹配。
- task `registry_generation` 与当前 registry generation 匹配。
- Effectful runner 只能 claim `effect.*` task。
- Committer runner 只能 claim `core.*` task。
- `effect.*` 只能由 Effectful runner claim。
- `core.*` 只能由 Committer runner claim。

排序：

```text
ready_at_step asc
priority desc
created_sequence asc
task_id asc
```

## 5. ResultRouter

Pure runner 不直接提交状态或执行副作用：

- `deltas` 生成 `core.commit` task。
- `events` 生成 `core.event.append` task。
- `effects` 生成 `effect.*` task。
- `values` / `resources` 记录 value/resource lineage。
- `tasks` 直接进入 TaskPool。

Committer runner 是 StateStore/EventLog 的唯一提交入口。

## 6. Resource / Value

Task payload 可包含：

- scalar immediate。
- 小型不可变 inline value。
- `ValueRef`。
- `ResourceRef`。
- `StateRef`。

默认一致性规则：

- 共享资源 readonly/sealed。
- 修改生成新 ref。
- 状态修改走 `StateDelta + expected_version`。
- 原地写必须 ExclusiveWriteLease。
- lease 过期、generation mismatch、provider 崩溃必须结构化失败。

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

- pending task 可以 rebind 到新 registry generation。
- clean / local dirty running invocation 应通过原 runner 的 cancel 管理面回到
  pending，再交给新 generation 重试。
- polluted / unknown dirty running invocation 必须保留旧 generation drain，或由上层
  提供明确 compensation；不得强行 dispose。
- removed surface 的 zero occupancy 判定必须来自 TaskPool、ResourceManager 等当前
  事实源，而不是手动缓存。
- effect occupancy 来自 pending/running `effect.*` task；stream occupancy 来自
  `ResourceAccess::Stream` 资源和显式 `SurfaceOccupancyHandle`；subscription/timer
  occupancy 来自显式 `SurfaceOccupancyHandle`。
- deprecated surface 禁止新增派生占用：task enqueue 必须检查 task kind、effect kind、
  runner hint 和 required surfaces；stream/subscription/timer 注册入口必须检查目标
  surface。

已经 orchestration 过的 raw input 不因新增 TaskDemand 自动重新 fan-out；补跑必须显式
生成 migration/backfill task。

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
