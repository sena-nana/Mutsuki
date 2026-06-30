# Repository Boundary

MutsukiPythonRunnerKit is split out of Mutsuki Core so Python code can evolve as
a language kit while Core remains a Rust-first runtime kernel.

## Owned Here

- Python SDK facade for plugin authors.
- Python runner backend and runner loop glue.
- Runner Link codec/transport implementation.
- Python mirrors of Core runtime contracts.
- Manifest parsing, validation, and export helpers.
- Fake Core and conformance tests for Runner Link compatibility.

## Not Owned Here

- Core TaskPool, RunnerRegistry, ResultRouter, StateStore, ResourceManager,
  EventLog, TraceLog, load-plan authority, or hot reload policy.
- Host lifecycle, Python environment creation, plugin install/update, or Tauri
  integration.
- Agent-specific protocol sugar, LLM sessions, memory, workflow orchestration,
  broadcast, marketplace, or domain providers.

## Dependency Direction

```text
Mutsuki Core contracts
  -> Runner Link wire shape
  -> MutsukiPythonRunnerKit mirror and SDK
  -> Python plugin code
```

Host repositories may depend on both Core and this kit to launch Python runners.
Core must not depend on this repository.
