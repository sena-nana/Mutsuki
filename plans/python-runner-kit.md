# Python Runner Kit

Python 端只保留 runner kit，不拥有 runtime kernel，也不提供第二套 TaskPool。

当前范围：

- Python mirror contracts。
- `PythonRunnerBackend`。
- `StdioJsonlBridge`，方法面为 `runner.step`、`runner.cancel`、
  `runner.dispose`。
- `PythonResourceManager`，支持 inline value、ValueRef、file-backed ResourceRef、
  copy-on-write 和 ExclusiveWriteLease。
- public API 面向 runner、resource descriptor 和 JSONL runner server。

验证从 `python/mutsuki-runtime-python` 运行：

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```
