# Refactor behavior matrix

This matrix is the functional compatibility boundary for the architecture refactor. Public Rust,
ABI, IPC, Web wire and product configuration shapes may change; the observable operations below
must remain available through the current typed owner after every phase.

| Surface | User-visible behavior that must remain | Evidence gate |
| --- | --- | --- |
| Runtime | Submit one task or a batch, wait for an outcome, cancel, drain, abort and reload without bypassing TaskPool | Runtime/Core/Host behavior tests and `cargo bench-smoke` |
| Service and CLI | Authenticate, inspect service/core/plugins/runners/tasks/health/logs, mutate supported lifecycle state and receive structured failures | Service control integration tests and CLI tests |
| Tauri | Load builtin/ABI plugins, invoke tasks, bridge events/resources and reject invalid packages before execution | Tauri host and ABI package integration tests |
| Web | Load extensions, perform authorized RPC, recover from disconnect and surface uncertain writes without a permanent busy state | Web SDK state-machine tests and WebHost integration tests |
| Link | Negotiate a typed session, reconnect, enforce frame budgets and preserve request identity | Link all-feature tests and performance smoke |
| Distributed | Place/localize/recover work through the sidecar without distributed types entering Core | Distributed tests, fault recovery and boundary script |
| Agent | Run model/tool/context/session/memory/knowledge flows through task and resource contracts | Agent conformance and plugin tests |
| Bot | Route events and commands, manage conversations/interactions, deliver idempotently and run platform adapters | Bot owner tests and fake platform E2E |
| Standard plugins | Execute resource, HTTP, filesystem, database, workflow and observe capabilities through declared effect/resource surfaces | Std plugin conformance and performance smoke |
| Template | Boot an empty product, assemble selected owner catalogs and fail loud when selected capability/config/secret is absent | Template tests, export and standalone clean-clone build |

## P1 acceptance additions

- ABI v1 artifacts are rejected before any business request; ABI v2 covers initialize,
  batch/cancel/dispose, task client and resource plan operations.
- Web hello/request deadlines, close cleanup, send rollback and connection generation fencing keep
  the current-generation pending set bounded and empty after terminal disconnect.
- Remote media validates every redirect and resolved address, stops on the first chunk crossing the
  byte budget, and propagates timeout/cancellation as structured failures.
- Bot SQLite state transitions use one initialized connection owner and atomic reserve/claim/CAS
  operations; 64 concurrent callers do not leak `SQLITE_BUSY` or duplicate ownership.
