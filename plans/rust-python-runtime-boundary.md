# Rust / Python Runtime Boundary

当前边界：

- Rust `CoreRuntime` 是 TaskPool、registry、state、trace、event 和 resource fact source。
- Python 不拥有 runtime kernel，不实现第二套 TaskPool。
- 外部 Python runner kit 通过 `PythonRunnerBackend` 和 `StdioJsonlBridge` 提供 runner 行为。
- 跨边界传递 `Task`、`RunnerContext`、`RunnerResult`、`ValueRef`、`ResourceRef` 等纯协议。
- 不跨边界传 Python object、Rust pointer、callable、socket、SDK client、数据库连接或真实 handle。

数据规则：

- small immutable value 可以 inline。
- 可共享或需版本控制的小数据进入 `ValueRef`。
- 大数据进入 `ResourceRef`，底层可为 mmap/blob/stream/provider RPC。
- 状态变更通过 `StateDelta + expected_version`，由 Rust core 的 commit task 提交。

JSONL runner 方法面：

```text
runner.run_batch(ctx, batch) -> CompletionBatch
runner.cancel
runner.dispose
```

JSONL task backend 方法面：

```text
task.submit_batch(batch) -> TaskHandle[]
```

旧 `runner.step`、`task.submit` 和任何单 task runner ABI 均不再是边界契约。
Python runner kit 可以在语言 API 层保留单 task submit/await 的用户体验，但必须在
bridge 内部生成 one-entry `TaskBatch`。

Python runner kit 的 public API 位于独立仓库，必须围绕上述方法面和 Rust contracts
mirror 展开。
