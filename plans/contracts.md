# MutsukiBot Runtime Contracts

本文件记录当前根级 Rust runtime 的内部协议。旧 Python `msgspec` contracts 已移动到
`python/reference-mutsukibot/`，作为参考与迁移材料，不再定义根级主链。

## 1. 协议总则

- 根级协议对象由 `crates/mutsuki-runtime-contracts` 定义，使用 serde
  serialize / deserialize。
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

## 7. Resource

- `ResourceGate` 管理 `RefDescriptor`、owner、lease token、lease count。
- `LeaseToken` 是绑定凭证；`token_id / ref_id / owner` 必须整体匹配才能 release。
- 伪造 token 或 stale token 必须结构化失败，并在 evidence 中记录
  `reason=lease_token_mismatch`（适用时）。
- 租约 token 由注入式 ID source 生成；不得直接调用全局 UUID / random。
- `list_records(owner)` 必须支持按 owner 过滤。

## 8. Trace

- Runtime 记录 lifecycle、input、strategy、operation 等 span。
- Span 必须包含 `trace_id`、`span_id`、可选 `parent_span_id`。
- Agent input 与 strategy span 必须能形成本地父子关系。
- 后续 Rust contract kit 应补齐完整 trace closure / replay 检查。

## 9. 标准错误码

| code | 场景 |
|---|---|
| `agent.not_found` | Agent 不存在 |
| `operation.not_found` | Operation 不存在 |
| `runtime.backend_failed` | backend 调用失败，且无法归入更具体错误 |
| `runtime.backend_generation_mismatch` | handler key generation 或 identity 不匹配 |
| `source.unregistered` | envelope source 未注册 |
| `scope.no_match` | 已注册 source 但无 awake + accepts 匹配 Agent |
| `ref.not_found` | resource / lease 不存在，或 token mismatch 归一化失败 |

## 10. Crate 对应

- `crates/mutsuki-runtime-contracts`：本文件的纯协议结构。
- `crates/mutsuki-runtime-core`：AgentRuntime、backend traits、ResourceGate、trace bookkeeping。
- `crates/mutsuki-runtime-host`：native in-memory host helper 和无 Python smoke。

## 11. 禁止事项

- Rust contracts / core 中不得出现外部协议 wire shape 或产品语义。
- 不得跨 runtime 边界传 callable、真实 handle、socket、SDK client、数据库连接或模型对象。
- 不得把未注册 Source 的 envelope 静默丢弃或隐式路由。
- 不得在 stale backend key 上 fallback 到新 handler。
