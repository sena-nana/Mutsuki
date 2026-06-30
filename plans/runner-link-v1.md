# Runner Link v1

Runner Link is the language-neutral protocol between Mutsuki Core and external
runners. This repository implements the Python side of that protocol.

## Layering

```text
transport: stdio now, named pipe / unix socket later
frame: JSONL debug path now, length-prefixed MessagePack later
codec: JSON-compatible contract objects
envelope/task: runner.step, runner.cancel, runner.dispose
sdk: ctx.call, ctx.resources, ctx.log, side-effect scope
```

## Current Implemented Surface

- Python dataclass mirrors for runtime contracts.
- JSON roundtrip helpers.
- `PythonRunnerBackend` runner registry and invocation.
- `StdioJsonlBridge` methods:
  - `runner.step`
  - `runner.cancel`
  - `runner.dispose`
  - resource read/write helper methods used by current tests.
- `PythonResourceManager` for descriptor-based test/resource flows.
- Runner-side async adapter for Mutsuki task awaitables.

## Required Invariants

- Python runner code never owns Core task state.
- `ctx.call` must lower to a child `Task` / `TaskAwait` flow handled by Core.
- Resource access must use `ResourceRef`, `ValueRef`, plans, leases, and
  structured generation checks.
- Unsupported Python awaitables must fail as `runner.awaitable_unsupported`.
- Any future MessagePack or frame support must preserve the same contract
  objects and JSON conformance cases.
