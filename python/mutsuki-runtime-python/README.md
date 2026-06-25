# Mutsuki Runtime Python

`mutsuki-runtime-python` is the Python runner kit for the Rust-first Mutsuki
TaskPool runtime.

It mirrors the Rust protocol objects and provides:

- `PythonRunnerHost`
- `StdioJsonlRunnerServer`
- `PythonResourceManager`
- test helpers for Python-owned runners

It is not a standalone runtime and does not depend on `python/reference-mutsuki`.
Python code provides runner behavior and host-owned resources; Rust `CoreRuntime`
remains the TaskPool, registry, state, trace, and event fact source.

## Checks

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```
