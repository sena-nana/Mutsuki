# Mutsuki Runtime Python

`mutsuki-runtime-python` is the Python backend kit for the Rust-first Mutsuki
runtime. It mirrors the Rust runtime contracts and provides an in-process
backend host for Python-owned strategy and operation handlers.

It is not a standalone Python runtime and does not depend on
`python/reference-mutsukibot`.

## Checks

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```
