# Mutsuki 工程实现规则

根目录当前是 Rust-first 极薄 single-task runtime framework。Python 端只保留当前
`python/mutsuki-runtime-python/` runner kit。

## 1. 技术栈

- Rust 2024 + Cargo workspace。
- serde / serde_json 用于纯协议序列化。
- thiserror 用于 runtime failure wrapper。
- Python 3.13+ + uv 用于 `python/mutsuki-runtime-python/`。

Rust crates 禁止依赖 Python、PyO3、产品协议 SDK、LLM provider 或领域语义。

## 2. 目录结构

```text
Mutsuki/
  Cargo.toml
  crates/
    mutsuki-runtime-contracts/  # Task / TaskLease / Runner / Resource / Plugin load-plan protocol
    mutsuki-runtime-core/       # CoreRuntime / TaskPool / TaskLease / Executor dispatch / ResourceManager
    mutsuki-runtime-host/       # native runner host / JSONL runner client
    mutsuki-runtime-sdk/        # Rust SDK async/await wrapper over TaskHandle
  plans/
  python/
    mutsuki-runtime-python/     # Python runner kit and protocol mirror
```

## 3. Crate 边界

- `mutsuki-runtime-contracts`：只定义纯数据结构，不包含 callable、socket、SDK client、
  真实 handle 或领域对象。
- `mutsuki-runtime-core`：实现 TaskPool、TaskLease、RunnerRegistry、Executor dispatch、
  ResultRouter、StateStore、ResourceManager、EventLog、TraceLog、hot-reload surface checks。
  Runner dispatch 可通过 `RunnerExecutor` 边界替换；core 默认只提供同步 inline
  执行器，不绑定线程模型。
- `mutsuki-runtime-host`：实现 native PluginHost/resolver、native runner wrapper 和
  stdio JSONL runner client，并提供默认 CoreActor / worker pool 隔离的
  `HostRuntime` 控制面门面。`NativePluginHost::into_runtime` 仍返回裸 `CoreRuntime`
  用于单线程测试、replay 和最小 host。
- `mutsuki-runtime-sdk`：实现 Rust 插件作者侧 `RuntimeClient`、`TaskHandleFuture`、
  `AsyncRunnerContext` 和 `AsyncRunnerAdapter`；不得把 async runtime 语义反向写入 Core。
- `python/mutsuki-runtime-python`：镜像协议，提供 Python runner host、stdio runner
  server、Python ResourceManager 测试实现、runner-side async adapter 和 typed public API。

## 4. 验证

根级 Rust 改动必须运行：

```powershell
cargo fmt --check
cargo test
```

改动 Python backend kit 时，从 `python/mutsuki-runtime-python` 运行：

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

不得用部分检查宣称成功。

## 5. 横切公约

- TaskPool 是 ready task backlog / 调度索引，不是 Runner inbox。
- Task 一次只能通过一个 TaskLease 交给一个 Runner / Executor 执行。
- Runner 是逻辑处理器，不是物理执行单元；Executor 是物理执行槽位。
- Rust `Runner` 必须是 `Send`；默认 host runtime 会把普通 runner 移动到 worker
  线程执行，CoreActor 不能直接执行插件 handler。
- `RunnerDescriptor.execution_class` 只用于 host 选择执行池，不改变 core 调度语义。
- Waiting task 释放 worker / lease，但继续占 runner 逻辑 inflight 配额。
- 普通 runner 禁止直接副作用。
- StateStore 只能通过 `core.commit` task 修改。
- EventLog 只能通过 kernel event append 或 runtime 事件记录修改。
- Effectful runner 只处理 `effect.*` task。
- ResourceRef/ValueRef/ResourceCellRef/ResourceLease/StateRef 是跨边界 descriptor，不是语言对象引用。
- 长期资源状态归 ResourceManager / ResourceCell；runner 只能持有 step 期间的 ResourceLease。
- 具体资源数据读写归 backend / provider；ResourceManager 保留 descriptor、lease、
  occupancy 和 generation 事实源。
- Core 不提供 TaskGroup、WaitSet、pipeline、broadcast、matcher、actor 或 endpoint runtime 实体。
- Rust SDK 可以提供 `ctx.call(...).await`，但其 wire 语义必须落到普通 task、
  `TaskAwait`、`Waiting`、wake 和 `TaskOutcome`。
- JS/TS SDK 不在当前 workspace；不得添加未接 runtime driver 的占位 API。Python 当前
  仅限 `python/mutsuki-runtime-python` runner kit 内的 runner-side awaitable adapter，
  不作为独立业务 SDK，也不承诺调度任意 `asyncio` future。
- registry boot 后 freeze；能力变化必须走新 registry generation。
- 错误必须结构化，不能吞异常返回默认值。
- ID、时间、随机源必须可注入或由 runtime/host 控制。

## 6. Git 与范围

- 公共协议、core runtime、ResourceManager、PluginHost、热重载或目录边界变化，提交前必须检查 diff 范围。
- 不覆盖用户或其他 Agent 的已有改动。
- 历史 version report 保留历史事实，不要求随当前架构改写。
