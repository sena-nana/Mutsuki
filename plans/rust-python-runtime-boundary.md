# Rust / Python Runtime Boundary

本文件记录当前事实：根级 MutsukiBot 是 Rust-first runtime framework。Python 代码已
移动到 `python/legacy-mutsukibot/`，仅作为 legacy / reference material；它不是根级
主运行时。

## 1. 当前实现

- `crates/mutsuki-runtime-contracts`：纯协议结构与 `ScopeRuleSpec.matches(...)`。
- `crates/mutsuki-runtime-core`：`AgentRuntime`、backend traits、Operation /
  Source registry、routing、trace bookkeeping、`ResourceGate`。
- `crates/mutsuki-runtime-host`：native in-memory host helper，可不依赖 Python 跑通
  Agent loop。
- `python/legacy-mutsukibot`：旧 Python framework、reference extensions、tests、
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

## 4. Python Legacy Boundary

Legacy Python may be revived as an optional host/sidecar, but it must obey the
same backend boundary:

- Rust runtime never imports Python modules.
- Cross-boundary data is limited to serializable contracts, backend keys, resource
  descriptors, lease tokens, and JSON-like payloads.
- Python callable, `Handle[T]`, socket, SDK client, database connection, model object,
  tensor, KV cache, or any raw domain object must not cross into Rust runtime.
- Plugin reload / unload must invalidate old handler generations. Stale keys fail as
  `runtime.backend_generation_mismatch`.

## 5. Current Acceptance

The Rust-first framework is acceptable only when:

- `cargo test` passes at root.
- `mutsuki-runtime-host` demonstrates native Agent start/publish/tick/invoke/stop without Python.
- Source registry rejects unregistered envelope sources.
- Resource leases reject forged token triples.
- Lease token generation is runtime-owned, not a global UUID call.
- Trace spans preserve at least local parent-child relationships for Agent input and strategy.
- Rust crates remain domain-neutral.

## 6. Future Optional Work

- Add an explicit process/RPC boundary for Python sidecar only if legacy Python capability hosting
  is needed.
- Add cancellation/deadline propagation across backend calls.
- Add Rust trace replay / contract kit parity with the old Python testing helpers.
- Add resource quota policies and capacity errors.
