# Mutsuki Runtime Python

`mutsuki-runtime-python` is the Python backend kit for the Rust-first Mutsuki
runtime. It mirrors the Rust runtime contracts and provides an in-process
backend host for Python-owned strategy and operation handlers.

It is not a standalone Python runtime and does not depend on
`python/reference-mutsukibot`.

Python can participate in two roles:

- runtime caller: external Python plugin entries may publish events or query a
  Rust runtime through future `runtime.*` client APIs.
- capability backend: Rust runtime can call Python-owned strategy hooks,
  operations, source providers, and resource hosts through `backend.*` methods.

In both roles, Rust `AgentRuntime` remains the only runtime kernel. Python keeps
plugin behavior, real Python-owned resources, external protocol adapters, and
structured error mapping; it does not own routing, lifecycle, inbox ticks,
runtime registry facts, ResourceGate decisions, trace, or event sequence.

## Checks

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```
