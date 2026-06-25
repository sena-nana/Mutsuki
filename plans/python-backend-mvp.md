# Python Runner Kit MVP

早期 Python backend MVP 已被 Python runner kit 取代。

当前 MVP：

- Python mirror contracts。
- `PythonRunnerHost`。
- `StdioJsonlRunnerServer`，方法面为 `runner.step`、`runner.cancel`、
  `runner.dispose`。
- `PythonResourceManager`，支持 inline value、ValueRef、file-backed ResourceRef、
  copy-on-write 和 ExclusiveWriteLease。
- public API 不导出早期 backend 兼容层。

验证从 `python/mutsuki-runtime-python` 运行：

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```
