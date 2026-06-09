# Rust / Python Runtime Boundary

本文件记录当前事实：根级 MutsukiBot 是 Rust-first runtime framework。Python 代码已
移动到 `python/reference-mutsukibot/`，作为旧 Python 实现的参考与迁移层；它不是
根级主运行时，也不代表废弃内容。

## 1. 当前实现

- `crates/mutsuki-runtime-contracts`：纯协议结构与 `ScopeRuleSpec.matches(...)`。
- `crates/mutsuki-runtime-core`：`AgentRuntime`、backend traits、Operation /
  Source registry、routing、trace bookkeeping、`ResourceGate`。
- `crates/mutsuki-runtime-host`：native in-memory host helper，可不依赖 Python 跑通
  Agent loop。
- `python/mutsuki-runtime-python`：新版 Python backend kit，镜像 Rust contracts，提供
  进程内 Python backend host 与 descriptor-only resource backend。
- `python/reference-mutsukibot`：旧 Python framework、reference extensions、tests、
  docs、examples。

## 2. Rust 主链

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
  -> Runtime trace/resource state updates
```

Rust runtime 是 lifecycle、routing、registry、resource lease 与 trace 事实源。Host
只提供策略和能力实现。

## 3. Backend Traits

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

## 4. Python Backend Kit Boundary

`python/mutsuki-runtime-python` 是当前 Python 端 MVP。它必须遵守：

- Rust runtime never imports Python modules.
- Python contracts mirror Rust serde wire shape; Rust contracts remain the source of truth.
- Python-owned callable stays inside `PythonBackendHost`; snapshots only expose descriptor,
  status, and `OperationHandlerKey`.
- Resource backend only tracks descriptor, owner, lease token, and lease count.
- Lease token IDs come from an injected ID source, not global time / UUID / random.
- Stale operation keys fail as `runtime.backend_generation_mismatch`.

## 5. Python Reference Boundary

The Python reference layer may be used to build an optional host/sidecar, but it
must obey the same backend boundary:

- Rust runtime never imports Python modules.
- Cross-boundary data is limited to serializable contracts, backend keys, resource
  descriptors, lease tokens, and JSON-like payloads.
- Python callable, `Handle[T]`, socket, SDK client, database connection, model object,
  tensor, KV cache, or any raw domain object must not cross into Rust runtime.
- Plugin reload / unload must invalidate old handler generations. Stale keys fail as
  `runtime.backend_generation_mismatch`.

## 6. Current Acceptance

The Rust-first framework is acceptable only when:

- `cargo test` passes at root.
- `mutsuki-runtime-host` demonstrates native Agent start/publish/tick/invoke/stop without Python.
- Source registry rejects unregistered envelope sources.
- Resource leases reject forged token triples.
- Lease token generation is runtime-owned, not a global UUID call.
- Trace spans preserve at least local parent-child relationships for Agent input and strategy.
- Rust crates remain domain-neutral.

## 7. Future Optional Work

- Add an explicit process/RPC boundary for Python sidecar only after the in-process backend kit
  contract is stable.
- Add cancellation/deadline propagation across backend calls.
- Add Rust trace replay / contract kit parity with the old Python testing helpers.
- Add resource quota policies and capacity errors.
