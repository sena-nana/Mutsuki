# Rust / Python Runtime Boundary

本文件记录当前事实：根级 MutsukiBot 是 Rust-first runtime framework。Python 代码已
移动到 `python/reference-mutsukibot/`，作为旧 Python 实现的参考与迁移层；它不是
根级主运行时，也不代表废弃内容。

## 1. 当前实现

- `crates/mutsuki-runtime-contracts`：纯协议结构与 `ScopeRuleSpec.matches(...)`。
- `crates/mutsuki-runtime-core`：`AgentRuntime`、backend traits、Operation /
  Source registry、routing、trace bookkeeping、runtime event stream、election policy、
  `ResourceGate`。
- `crates/mutsuki-runtime-host`：native in-memory host helper，可不依赖 Python 跑通
  Agent loop；另提供 stdio JSONL backend adapter。
- `python/mutsuki-runtime-python`：新版 Python backend kit，镜像 Rust contracts，提供
  进程内 Python backend host、descriptor-only resource backend 与 stdio JSONL server。
- `python/reference-mutsukibot`：旧 Python framework、reference extensions、tests、
  docs、examples。

## 2. 三层结构

长期结构按三个角色划分：

```text
Caller
  -> Rust Runtime Kernel
  -> Capability Backend
```

- Caller 是运行时调用方，可以是 Rust 应用、Python 插件入口、HTTP 服务、CLI 或其他
  项目。Caller 只能通过 runtime API 控制或查询 Rust runtime。
- Rust Runtime Kernel 是唯一 runtime 事实源，负责 Agent lifecycle、routing、inbox、
  Source / Operation registry snapshot、ResourceGate、trace 和 runtime event stream。
- Capability Backend 是能力宿主，可以是 native Rust host、Python sidecar、旧
  Python reference 的迁移 adapter 或其他远程服务。它只提供策略、Operation handler、
  Source snapshot 和资源对象访问能力。

Python 不是第二个 runtime kernel。`python/mutsuki-runtime-python` 可以同时扮演
runtime caller 与 capability backend：作为 caller 时向 Rust runtime 发布外部事件或查询
状态；作为 backend 时被 Rust runtime 调用 Python-owned handler 和 resource host。

## 3. 协议平面

跨边界通信分成两个平面：

- `runtime.*`：Caller -> Rust Runtime Kernel。用于注册/启动 Agent、发布 envelope、
  查询状态、drain events、调用 runtime-owned operation 路径等。该平面是未来
  runtime-control protocol 的归属，本文件只锁定边界，不要求当前实现新增 RPC。
- `backend.*`：Rust Runtime Kernel -> Capability Backend。用于 `on_awake`、`on_input`、
  `next_step`、`list_operations`、`list_sources`、`invoke`、`operation_status` 和
  `resource.*`。当前 stdio JSONL 边界属于该平面。

`runtime.*` 和 `backend.*` 可以复用同一 request/response 外壳、`RuntimeError` 和 serde
contracts，但语义不能混用。Caller 不直接调用 backend API；backend handler 不拥有
runtime 调度事实。

## 4. 往返调用规则

Python 和 Rust 之间允许反复调用，但 v1 采用“混合但受限”模型：

- `runtime.query.*` 可以同步调用，只读查询不得推进 lifecycle、routing、tick 或 resource
  lease 状态。
- `runtime.command.*` 会改变 runtime 状态，默认进入 Rust runtime 队列或 turn 边界处理。
- `runtime.events.*` 用于 snapshot、drain 或订阅 runtime event stream。
- backend handler 执行期间不得同步重入同一个 Rust runtime 的 `tick_once`、`start_agent`、
  `stop_agent` 等状态推进 API。
- backend handler 如需触发后续输入，应返回 emitted envelope 或调用受限的 enqueue/publish
  command，让 Rust runtime 在当前 backend 调用结束后推进。
- 跨边界调用应携带 request id、trace context 和 deadline；未来 transport 可在不改变
  contracts 的前提下补充这些元信息。

## 5. Rust 主链

```text
Envelope
  -> AgentRuntime.publish
  -> Source registry validation
  -> Agent.accepts matching
  -> Agent inbox
  -> AgentRuntime.tick_once / tick loop
  -> StrategyBackend.on_input or next_step
  -> OperationBackend.invoke by OperationHandlerKey
  -> StrategyResult
  -> Runtime trace/resource/event state updates
```

Rust runtime 是 lifecycle、routing、registry、resource lease 与 trace 事实源。Host
只提供策略和能力实现。

## 6. Backend Traits

### StrategyBackend

```text
on_awake(agent_id) -> Result<(), RuntimeFailure>
on_input(agent_id, envelope) -> Result<StrategyResult, RuntimeFailure>
next_step(agent_id) -> Result<StrategyResult, RuntimeFailure>
on_stop(agent_id) -> Result<(), RuntimeFailure>
```

### OperationBackend

```text
list_operations(agent_id) -> Result<Vec<OperationSnapshot>, RuntimeFailure>
list_sources(agent_id) -> Result<Vec<SourceSnapshot>, RuntimeFailure>
invoke(agent_id, key, payload) -> Result<BackendPayload, RuntimeFailure>
operation_status(agent_id, key) -> OperationStatus
```

### ResourceBackend

```text
register_resource(descriptor, owner) -> Result<RefId, RuntimeFailure>
acquire_resource(ref_id, requester) -> Result<LeaseToken, RuntimeFailure>
release_resource(token) -> Result<(), RuntimeFailure>
list_records(owner) -> Vec<ResourceRecord>
```

## 7. Python Backend Kit Boundary

`python/mutsuki-runtime-python` 是当前 Python 端 MVP。它必须遵守：

- Rust runtime never imports Python modules.
- Python contracts mirror Rust serde wire shape; Rust contracts remain the source of truth.
- Python-owned callable stays inside `PythonBackendHost`; snapshots only expose descriptor,
  status, and `OperationHandlerKey`.
- Resource backend only tracks descriptor, owner, lease token, and lease count.
- Lease token IDs come from an injected ID source, not global time / UUID / random.
- Stale operation keys fail as `runtime.backend_generation_mismatch`.
- Python keeps plugin metadata compilation, operation handlers, strategy hooks, source
  providers, Python-owned real resource objects/finalizers, external protocol adapters, and
  Python exception to `RuntimeError` mapping.
- Python does not own Agent lifecycle, routing / accepts, inbox / tick, runtime registry
  facts, ResourceGate quota decisions, trace/event sequence, or cross-agent scheduling.

## 8. Python Reference Boundary

The Python reference layer may be used to build an optional host/sidecar, but it
must obey the same backend boundary:

- Rust runtime never imports Python modules.
- Cross-boundary data is limited to serializable contracts, backend keys, resource
  descriptors, lease tokens, and JSON-like payloads.
- Python callable, `Handle[T]`, socket, SDK client, database connection, model object,
  tensor, KV cache, or any raw domain object must not cross into Rust runtime.
- Plugin reload / unload must invalidate old handler generations. Stale keys fail as
  `runtime.backend_generation_mismatch`.
- Reference-layer runtime responsibilities should migrate toward Rust runtime semantics:
  Agent lifecycle, routing / accepts, inbox / tick, Operation / Source runtime registry,
  resource lease gate, trace/event sequence, and cross-agent scheduling.
- Reference-layer plugin behavior, transport adapters, real resource ownership, and migration
  helpers may remain in Python and be exposed through backend snapshots and keys.

## 9. Current Acceptance

The Rust-first framework is acceptable only when:

- `cargo test` passes at root.
- `mutsuki-runtime-host` demonstrates native Agent start/publish/tick/invoke/stop without Python.
- Source registry rejects unregistered envelope sources.
- Resource leases reject forged token triples and enforce configured `ref_id` / `kind` quotas
  as `capability.exhausted`.
- Lease token generation is runtime-owned, not a global UUID call.
- Trace spans preserve at least local parent-child relationships for Agent input and strategy.
- Runtime events expose lifecycle / routing / operation / resource facts as pure contracts.
- Rust crates remain domain-neutral.

## 10. Future Optional Work

- Define `runtime.*` client/server contracts for remote Rust runtime execution.
- Add a local `RuntimeClient` facade that can wrap either embedded `AgentRuntime` or a remote
  Rust runtime service.
- HTTP or long-running Python sidecar supervision on top of the current stdio JSONL boundary.
- Add cancellation/deadline propagation across backend calls.
- Add Rust trace replay / contract kit parity with the old Python testing helpers.
- Add more resource quota dimensions beyond current `ref_id` / `kind` limits.
