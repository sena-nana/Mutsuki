# Mutsuki Python Runner Kit

`mutsuki-python-runner-kit` is the Python SDK and runner glue for Mutsuki
plugins that execute outside the Rust core. It implements the Python side of the
language-neutral Runner Link contract exposed by Mutsuki Core.

The distribution name is `mutsuki-python-runner-kit`; Python code imports
`mutsuki_runner_kit`.

## Boundary

This repository owns:

- Python mirrors of the Mutsuki runtime contracts.
- `PythonRunnerBackend` for runner registration, invocation, cancel, and dispose.
- Concurrent typed `StdioJsonlBridge` for compatibility/debugging and
  `StdioBinaryBridge` for length-prefixed MessagePack Runtime Wire v1.
- descriptor-based resource clients and an explicitly injected resource request handler.
- `FakeResourceProvider` under `testing` for conformance tests only.
- Testing helpers for Python-owned runners.

This repository does not own:

- The Mutsuki Core TaskPool, scheduler, registry, state store, trace log, or
  event log.
- Host environment startup, plugin installation, virtualenv management, or
  application lifecycle.
- Agent-specific memory, LLM session, workflow, broadcast, or marketplace logic.

## Checks

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

The Rust core repository remains the source of truth for protocol evolution.
When contracts change, update this kit only by mirroring the published wire
shape; do not add Python-only runtime semantics.

The checked-in schema and semantic fixtures are pinned to MutsukiCore
`05dc54cd63dc443eb4599f8932e90a2f928e307c`. Startup rejects an incompatible
protocol major, codec, or schema revision before runner work is dispatched.
Performance measurements and JSONL operational limits are documented in
`docs/performance/runtime-wire-v1.md`.

The unified Python Runner performance entrypoint is:

```text
uv run python benchmarks/performance_model.py --mode smoke --output artifacts/perf/python.json
```

ServiceHost and external orchestrators start `benchmarks/fixture_process.py` with either
`--codec python-jsonl` or `--codec python-binary`. These are real stdio processes; codec-only,
in-memory, real-pipe and external ServiceHost results remain separate measurement boundaries.
