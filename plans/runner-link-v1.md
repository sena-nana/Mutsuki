# Runner Link v1

当前 Python contract mirror 对齐 MutsukiCore revision
`aa5bd7a2cf0a8ba5065392d324836dadab5d7d70`（Issue #25 observability contract）。

Runner Link is the language-neutral protocol between Mutsuki Core and external
runners. This repository implements the Python side of that protocol.

## Layering

```text
transport: stdio now, named pipe / unix socket later
frame: JSONL debug path now, length-prefixed MessagePack later
codec: JSON-compatible contract objects
envelope/task: runner.run_batch, runner.cancel, runner.dispose
sdk: ctx.call, ctx.resources, ctx.log, side-effect scope
```

## Current Implemented Surface

- Python dataclass mirrors for runtime contracts, including batch-first shapes:
  `BatchEntry`, `WorkBatch`, `CompletionBatch`, `EntryCompletion`,
  `BatchPayload`, `WorkResourcePlan`, and `TaskBatch`.
- JSON roundtrip helpers.
- `PythonRunnerBackend` runner registry and invocation.
- `StdioJsonlBridge` methods:
  - `runner.run_batch`
  - `runner.cancel`
  - `runner.dispose`
  - resource read/write helper methods used by current tests.
- Explicit resource request handler injection; the bundled provider implementation is testing-only.
- Runner-side async adapter and scalar `run_one` adapter sugar that lower to
  `run_batch`.

## Required Invariants

- Python runner code never owns Core task state.
- Wire ABI is batch-first: `runner.run_batch({ runner_id, ctx, batch }) ->
  CompletionBatch`. Scalar `run_one` exists only as adapter sugar.
- `ctx.call` must lower to a child `Task` / `TaskAwait` flow handled by Core.
- Resource access must use `ResourceRef`, `ValueRef`, plans, leases, and
  structured generation checks.
- Unsupported Python awaitables must fail as `runner.awaitable_unsupported`.
- Any future MessagePack or frame support must preserve the same contract
  objects and JSON conformance cases.
