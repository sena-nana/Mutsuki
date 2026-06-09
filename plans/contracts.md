# MutsukiBot Runtime Contracts

本文件记录当前根级 Rust runtime 的内部协议。旧 Python `msgspec` contracts 已移动到
`python/reference-mutsukibot/`，作为参考与迁移材料，不再定义根级主链。

## 1. 协议总则

- 根级协议对象由 `crates/mutsuki-runtime-contracts` 定义，使用 serde
  serialize / deserialize。
- `python/mutsuki-runtime-python` 镜像这些协议对象的 Python wire shape，但 Rust
  contracts 仍是事实源。
- 协议对象必须是纯数据，不包含 callable、socket、SDK client、真实 handle 或领域对象。
- Rust core 只解释通用 runtime 字段，不解释领域 payload、resource attributes 或外部 wire shape。
- 失败使用 `RuntimeError` + `RuntimeFailure`，错误码必须稳定可断言。

## 2. 核心对象

| 对象 | 关键字段 / 语义 |
|---|---|
| `AgentSpec` | `agent_id`、`owner`、`priority`、`participation`、`accepts`、`strategy_id`、`side_effect_policy` |
| `AgentPhase` | `spawn`、`awake`、`sleep`、`stop` |
| `AgentParticipation` | `primary_candidate`、`observer`、`explicit_helper` |
| `Envelope` | `id`、`timestamp`、`source`、`payload_schema_id`、`capabilities_required`、`payload` |
| `SourceRef` | `source_id`、`kind`、`metadata` |
| `ScopeRuleSpec` | `always`、`never`、`all`、`any`、`by_schema`、`by_schema_prefix`、`by_source_id`、`by_source_kind`、`by_capability`、`by_source_field` |
| `OperationDescriptor` | `op_id`、`name`、`description`、`plugin_id`、schema、permission/capability metadata、`is_tool` |
| `SourceDescriptor` | `source_id`、`kind`、`capabilities`、`description` |
| `OperationHandlerKey` | `plugin_id`、`plugin_generation`、`op_id`、`handler_id` |
| `OperationSnapshot` | `descriptor`、`status`、`key` |
| `SourceSnapshot` | `descriptor`、`plugin_id`、`plugin_generation` |
| `StrategyResult` | `status`、`decision`、`emitted`、`error` |
| `RuntimeError` | `code`、`source`、`route`、`lost_capability`、`recovery`、`cause`、`evidence` |
| `TraceSpan` | `trace_id`、`span_id`、`parent_span_id`、`name`、`start`、`end`、`attributes`、`status` |
| `RuntimeEvent` | `sequence`、`kind`、`name`、`agent_id`、`attributes`、`error`；v1 不含 wall-clock timestamp |
| `RefDescriptor` | `ref_id`、`kind`、`schema_id_target`、`schema_version_target`、`attributes`、`lineage` |
| `LeaseToken` | `token_id`、`ref_id`、`owner` |
| `ResourceRecord` | `descriptor`、`owner`、`lease_count` |

## 3. Backend Traits

Runtime 与 host 通过 trait 通信。Host 可以是 native Rust，也可以是后续 sidecar
adapter；Rust core 不保存真实 handler。

```text
StrategyBackend:
  on_awake(agent_id) -> Result<(), RuntimeFailure>
  on_input(agent_id, envelope) -> Result<StrategyResult, RuntimeFailure>
  next_step(agent_id) -> Result<StrategyResult, RuntimeFailure>
  on_stop(agent_id) -> Result<(), RuntimeFailure>

OperationBackend:
  list_operations(agent_id) -> Result<Vec<OperationSnapshot>, RuntimeFailure>
  list_sources(agent_id) -> Result<Vec<SourceSnapshot>, RuntimeFailure>
  invoke(agent_id, key, payload) -> Result<BackendPayload, RuntimeFailure>
  operation_status(agent_id, key) -> OperationStatus

ResourceBackend:
  register_resource(descriptor, owner) -> Result<RefId, RuntimeFailure>
  acquire_resource(ref_id, requester) -> Result<LeaseToken, RuntimeFailure>
  release_resource(token) -> Result<(), RuntimeFailure>
  list_records(owner | None) -> Vec<ResourceRecord>
```

## 4. Lifecycle

- `register_agent(spec)` 创建 `spawn` 状态 Agent。
- `start_agent(agent_id, backend)` 使用提交语义：
  - `backend.on_awake` 成功。
  - `backend.list_operations` 成功并提交 operation registry。
  - `backend.list_sources` 成功并提交 source registry。
  - 全部成功后才进入 `awake`。
- 启动失败时 Agent 保持非路由状态；operation/source registry 不提交为可用事实。
- `stop_agent` 进入 `sleep`，调用 `backend.on_stop` 后进入 `stop`。

## 5. Routing

- `publish(envelope)` 首先校验 `envelope.source.source_id` 已在任一 Source registry
  中注册。
- 未注册 Source 必须返回 `source.unregistered`。
- Source 已注册后，runtime 只投递给 `phase == awake` 且任一 `accepts` 匹配的 Agent。
- `accepts` 为空等价于不接收任何 envelope。
- `select_accepting(envelope)` 同样要求 source 已注册；未注册 source 返回空选择。
- Source 已注册后，`select_accepting(envelope)` 只从 `primary_candidate` 中选择
  owner，排序为 priority 降序、`agent_id` 升序。

## 6. Operation

- Runtime 持有 `OperationSnapshot`，不持有 handler。
- 调用路径为 `invoke_operation(agent_id, op_id, payload, backend)`。
- op 不存在返回 `operation.not_found`。
- snapshot 非 `active` 返回结构化 backend failure，并记录 operation status。
- backend 必须验证 `OperationHandlerKey`；旧 generation 或 key mismatch 必须返回
  `runtime.backend_generation_mismatch`。
- `operation_status` trait 不返回 structured failure；JSONL host adapter 在 transport /
  protocol / backend 查询失败时必须降级为 `unhealthy`，不得伪装为 `not_found`。

## 7. Resource

- `ResourceGate` 管理 `RefDescriptor`、owner、lease token、lease count。
- `ResourceGate` 可配置 `ResourceQuotaPolicy`：`max_leases_by_ref` 与
  `max_leases_by_kind`；acquire 时先检查 `ref_id`，再检查 resource `kind`。
- `max_leases_by_kind` 表示同一 resource `kind` 的活跃 lease 总量上限，不是单个
  `ref_id` 的局部计数。
- 容量耗尽必须返回 `capability.exhausted`，并在 evidence 记录 `dimension`、
  `ref_id`、`kind`、`current`、`max`、`requester`；失败 acquire 不得增加租约计数。
- `LeaseToken` 是绑定凭证；`token_id / ref_id / owner` 必须整体匹配才能 release。
- 伪造 token 或 stale token 必须结构化失败，并在 evidence 中记录
  `reason=lease_token_mismatch`（适用时）。
- 租约 token 由注入式 ID source 生成；不得直接调用全局 UUID / random。
- `list_records(owner)` 必须支持按 owner 过滤。

## 8. Trace

- Runtime 记录 lifecycle、input、strategy、operation 等 span。
- Span 必须包含 `trace_id`、`span_id`、可选 `parent_span_id`。
- Agent input 与 strategy span 必须能形成本地父子关系。
- Rust trace closure helper 必须能报告重复 span、父链缺失、父子 trace 不一致和
  `end < start` 的时间区间错误。

## 9. Runtime Event Stream

- Runtime event stream 使用 `RuntimeEvent` 纯协议记录 lifecycle / routing / operation /
  resource / trace / backend 事件。`kind` 表示事件分类，`name` 表示稳定事件名。
- `sequence` 由 runtime 全局分配，是确定性递增顺序；`drain_events()` 清空已读事件，
  但不得重置或复用 sequence。不要求 v1 使用 wall-clock timestamp。
- Trace 事实源仍是 `TraceSpan`；每次 runtime 记录 span 时同步产生一个
  `kind=trace`、`name=trace.span` 的事件。该事件 `attributes` 至少包含 `trace_id`、
  `span_id`、`span_name`、`start`、`end`（span 有 end 时）与 `status`，有父 span 时
  包含 `parent_span_id`。
- Resource events 属于 `AgentRuntime` event stream。runtime-owned `ResourceGate` 可以
  暂存内部 event draft，下一次 snapshot / drain / runtime event emit 时由
  `AgentRuntime` 统一分配 sequence；standalone `ResourceGate` 不维护可观察 event stream，
  也不收集 draft。
- `tick_once` 调用 backend 后按 outcome 记录事件；backend `Err` 或 `StrategyResult.error`
  必须记录为 `agent.input.error` / `agent.next_step.error` 并携带 structured error，不得
  先写入 `error=None` 的成功事件。
- Resource acquire / release 的成功和失败都必须记录为 `resource.*` /
  `resource.*.error`；失败事件必须携带 structured `RuntimeError`。
- 事件可以携带 scalar attributes 与结构化 `RuntimeError`，但不得包含 callable、真实
  handle、socket、SDK client 或领域对象。
- Host 可通过 runtime 暴露的 event snapshot / drain API 获取事件；事件失败不得改变
  主链路语义。

## 10. Election Policy

- 默认 election policy 保持 `priority` 降序、`agent_id` 升序。
- 可替换 policy 只能排序 runtime 已过滤的候选：source 已注册、agent awake、accepts
  匹配且 participation 为 `primary_candidate`。
- policy 返回非候选 agent 时视为未选择，不得绕过 lifecycle / accepts 过滤。

## 11. Python Stdio JSONL Boundary

- `python/mutsuki-runtime-python` 提供 stdio JSONL 进程边界，复用纯 contracts 与
  backend key。
- 请求形状：`{"id":"req-1","method":"invoke","params":{...}}`。
- 成功响应：`{"id":"req-1","ok":true,"result":...}`。
- 失败响应：`{"id":"req-1","ok":false,"error": RuntimeError}`。
- 支持方法：`on_awake`、`on_input`、`next_step`、`on_stop`、`list_operations`、
  `list_sources`、`invoke`、`operation_status`、`resource.register`、
  `resource.acquire`、`resource.release`、`resource.list`。
- Rust core 不依赖 Python；Rust host 的 JSONL adapter 只依赖泛型 line IO 与 serde JSON。

## 12. Future Runtime-Control Boundary

未来若将 Rust runtime 作为远程执行内核暴露给其他项目，必须新增独立的
runtime-control protocol。该协议属于 Caller -> Rust Runtime Kernel，不得复用
`backend.*` 方法名来表达 runtime 调度。

v1 只记录边界，不要求当前代码实现新 RPC。建议命名空间：

- `runtime.query.*`：同步只读查询，例如 agent phase、operation/source snapshot、
  event snapshot、trace snapshot、resource records。
- `runtime.command.*`：会改变 runtime 状态的命令，例如 register/start/stop agent、
  publish envelope、tick、invoke operation、resource acquire/release。默认进入 Rust
  runtime queue 或 turn 边界处理。
- `runtime.events.*`：event snapshot、drain 或订阅。
- `backend.*`：仍只表示 Rust Runtime Kernel -> Capability Backend，包括当前
  `on_awake`、`on_input`、`next_step`、`invoke` 和 `resource.*` backend 方法。

`runtime.*` 与 `backend.*` 可以共享 request id、trace context、deadline、JSON
request/response 外壳和 `RuntimeError`，但不能共享状态所有权。Rust runtime 是
lifecycle、routing、registry、ResourceGate、trace 和 event sequence 的唯一事实源；
backend handler 不得同步重入同一个 runtime 的状态推进 API。

## 13. 标准错误码

| code | 场景 |
|---|---|
| `agent.not_found` | Agent 不存在 |
| `operation.not_found` | Operation 不存在 |
| `runtime.backend_failed` | backend 调用失败，且无法归入更具体错误 |
| `runtime.backend_generation_mismatch` | handler key generation 或 identity 不匹配 |
| `source.unregistered` | envelope source 未注册 |
| `scope.no_match` | 已注册 source 但无 awake + accepts 匹配 Agent |
| `ref.not_found` | resource / lease 不存在，或 token mismatch 归一化失败 |
| `capability.exhausted` | resource quota 或 capability 门控耗尽 |

## 14. Crate 对应

- `crates/mutsuki-runtime-contracts`：本文件的纯协议结构。
- `crates/mutsuki-runtime-core`：AgentRuntime、backend traits、ResourceGate、trace bookkeeping。
- `crates/mutsuki-runtime-host`：native in-memory host helper 和无 Python smoke。
- `python/mutsuki-runtime-python`：Python backend kit，提供 contracts mirror、进程内
  backend host、resource backend 和测试夹具；不定义 Rust runtime 事实。

## 15. 禁止事项

- Rust contracts / core 中不得出现外部协议 wire shape 或产品语义。
- 不得跨 runtime 边界传 callable、真实 handle、socket、SDK client、数据库连接或模型对象。
- 不得把未注册 Source 的 envelope 静默丢弃或隐式路由。
- 不得在 stale backend key 上 fallback 到新 handler。
