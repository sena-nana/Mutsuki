# Python Backend Kit MVP

本文件记录新版 Python 端的 MVP 边界。根级主链仍是 Rust-first runtime；Python
端只提供可被 Rust runtime / host 通过纯协议驱动的 backend kit。

## 1. 定位

- 包目录：`python/mutsuki-runtime-python/`
- 发行名：`mutsuki-runtime-python`
- 导入名：`mutsuki_runtime_python`

该包不是旧 Python framework 的回迁，也不是第二套 runtime。它只承载：

- Rust contracts 的 Python wire-shape 镜像。
- 进程内 `StrategyBackend` / `OperationBackend` / `ResourceBackend` 协议与实现。
- Python-owned operation handler、strategy hook、source provider 和 resource host 的
  测试夹具。
- stdio JSONL request/response server，作为第一版显式进程边界。

`PythonBackendHost` 的长期定位是 Python plugin / capability host：它可以保存
Python callable、插件 metadata、strategy hook 和 Source / Operation snapshot 的生成
材料，但不得拥有 routing、lifecycle、Agent inbox、runtime event sequence 或
ResourceGate quota 事实。Python 侧如需作为外部入口发布事件，应通过未来 `runtime.*`
caller API 进入 Rust runtime。

`python/reference-mutsuki/` 继续作为旧实现参考与迁移材料存在；新版包不得依赖
旧 `mutsuki` core、dispatcher、PluginLoader 或 extension。

## 2. MVP 结构

```text
python/mutsuki-runtime-python/
  pyproject.toml
  src/mutsuki_runtime_python/
    contracts.py   # Rust serde wire-shape mirror
    backend.py     # backend protocols and structured error wrapper
    host.py        # in-process PythonBackendHost
    resource.py    # descriptor-only resource lease backend
    stdio.py       # stdio JSONL backend server
    testing.py     # deterministic fixtures and smoke helper
  tests/
```

MVP 已实现进程内边界与 stdio JSONL 进程边界。HTTP RPC、取消、deadline 和长期
sidecar supervisor 后续再单独设计；当前 contracts、snapshot 和 backend key 形状必须
保持可映射到未来 RPC。

## 3. Contract Rules

- Python contracts 以当前 Rust `mutsuki-runtime-contracts` 为事实源。
- `ScopeRuleSpec` 使用 Rust 一致的 tagged JSON shape，例如
  `{"type":"by_schema","schema_id":"test.input"}`。
- 枚举值使用 `snake_case`，默认字段按 serde 行为补齐。
- operation snapshot 只能包含 descriptor、status 和 `OperationHandlerKey`；不得序列化
  Python callable。
- stale backend key 必须 fail-loud 为 `runtime.backend_generation_mismatch`，不得 fallback
  到新 handler。
- resource backend 只保存 `RefDescriptor`、owner、`LeaseToken` 和 lease count；真实对象、
  finalizer、socket、SDK client、数据库连接等不进入 runtime 边界。
- stdio JSONL response 必须用 `RuntimeError` 表达失败，不输出 raw traceback 作为协议字段。
- Python backend kit 不实现 `AgentRuntime`、routing、accepts matching、agent election、
  inbox tick 或 trace/event sequence；这些职责属于 Rust runtime。
- Python 插件可作为 runtime caller，但状态变更必须走 Rust runtime 的 command / queue
  边界，不能在 backend handler 内同步重入同一个 runtime 的状态推进 API。

## 4. Stdio JSONL Boundary

- 请求形状：`{"id":"req-1","method":"invoke","params":{...}}`。
- 成功响应：`{"id":"req-1","ok":true,"result":...}`。
- 失败响应：`{"id":"req-1","ok":false,"error": RuntimeError}`。
- 支持方法：`on_awake`、`on_input`、`next_step`、`on_stop`、`list_operations`、
  `list_sources`、`invoke`、`operation_status`、`resource.register`、
  `resource.acquire`、`resource.release`、`resource.list`。
- 该边界是 `backend.*` 语义：Rust runtime 调 Python capability host。未来
  Caller 调远程 Rust runtime 的 `runtime.*` control protocol 必须单独定义。

## 5. Verification

Python backend kit 改动需在 `python/mutsuki-runtime-python` 运行：

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

若同时改动 Rust contracts / core / host，仍需在仓库根目录运行：

```powershell
cargo test
```
