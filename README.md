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
- `StdioJsonlBridge` for the current JSONL runner bridge.
- `PythonResourceManager` and descriptor-based resource helpers used by Python
  runners and conformance tests.
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
