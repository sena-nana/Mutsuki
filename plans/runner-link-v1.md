# Runner Link v1

当前 Python contract mirror 与 Runtime Wire active release fixtures 对齐 MutsukiCore revision
`c6a68f8ab2c6450035580c12b5944e2a21ddf437`。

Runner Link is the language-neutral protocol between Mutsuki Core and external
runners. This repository implements the Python side of that protocol.

## Layering

```text
transport: concurrent stdio now, named pipe / unix socket later
frame: typed JSONL debug + length-prefixed MessagePack v1
codec: schema-validated typed contract objects
envelope/task: runner.run_batch, runner.cancel, runner.dispose
sdk: ctx.call, ctx.resources, ctx.log, side-effect scope
```

## Current Implemented Surface

- Python dataclass mirrors for runtime contracts, including batch-first shapes:
  `BatchEntry`, `WorkBatch`, `CompletionBatch`, `EntryCompletion`,
  `BatchPayload`, `WorkResourcePlan`, and `TaskBatch`.
- JSON roundtrip helpers.
- `PythonRunnerBackend` runner registry and invocation.
- `StdioJsonlBridge` typed Opcode methods:
  - `runner.run_batch`
  - `runner.cancel`
  - `runner.dispose`
  - resource read/write helper methods used by current tests.
- Explicit resource request handler injection; the bundled provider implementation is testing-only.
- Runner-side async adapter and scalar `run_one` adapter sugar that lower to
  `run_batch`.
- `StdioBinaryBridge` uses the same dispatcher and semantic fixtures with a fixed
  24-byte header and typed MessagePack payload.
- Initialization negotiates protocol major, codec, schema revision, features and
  limits before business dispatch.
- stdio reader schedules bounded concurrent work, reserves management capacity,
  supports out-of-order response correlation, and writes protocol frames only to stdout.

## Required Invariants

- Python runner code never owns Core task state.
- Wire ABI is batch-first: `runner.run_batch({ runner_id, ctx, batch }) ->
  CompletionBatch`. Scalar `run_one` exists only as adapter sugar.
- `ctx.call` must lower to a child `Task` / `TaskAwait` flow handled by Core.
- `RunnerResult.output` mirrors the small inline terminal result read from
  `TaskOutcome.completed.output`; large results remain provider-owned through `output_ref`.
- Resource access must use `ResourceRef`, `ValueRef`, plans, leases, and
  structured generation checks.
- Unsupported Python awaitables must fail as `runner.awaitable_unsupported`.
- JSONL and MessagePack preserve the same contract objects and Rust-generated
  active release conformance cases.
- JSONL is compatibility/debug only. Recommended operational limits are 64 KiB
  payload and 32 entries; hard limits remain schema-defined.
- Resource bytes over 64 KiB must use `ResourceRef`, stream, or shared descriptor.
