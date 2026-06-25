# Mutsuki 工程实现规则

根目录当前是 Rust-first TaskPool runtime framework。Python 端只保留当前
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
    mutsuki-runtime-contracts/  # Task / Runner / Resource / Plugin load-plan protocol
    mutsuki-runtime-core/       # CoreRuntime / TaskPool / RunnerLoop / ResourceManager
    mutsuki-runtime-host/       # native runner host / JSONL runner client
  plans/
  python/
    mutsuki-runtime-python/     # Python runner kit and protocol mirror
```

## 3. Crate 边界

- `mutsuki-runtime-contracts`：只定义纯数据结构，不包含 callable、socket、SDK client、
  真实 handle 或领域对象。
- `mutsuki-runtime-core`：实现 TaskPool、RunnerRegistry、RunnerLoop、ResultRouter、
  StateStore、ResourceManager、EventLog、TraceLog、hot-reload surface checks。
- `mutsuki-runtime-host`：实现 native PluginHost/resolver、native runner wrapper 和
  stdio JSONL runner client。
- `python/mutsuki-runtime-python`：镜像协议，提供 Python runner host、stdio runner
  server、Python ResourceManager 测试实现和 typed public API。

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

- TaskPool 是唯一待处理任务事实源。
- Runner 是唯一执行/编排/外部操作适配单元。
- 普通 runner 禁止直接副作用。
- StateStore 只能通过 `core.commit` task 修改。
- EventLog 只能通过 kernel event append 或 runtime 事件记录修改。
- Effectful runner 只处理 `effect.*` task。
- ResourceRef/ValueRef/StateRef 是跨边界 descriptor，不是语言对象引用。
- registry boot 后 freeze；能力变化必须走新 registry generation。
- 错误必须结构化，不能吞异常返回默认值。
- ID、时间、随机源必须可注入或由 runtime/host 控制。

## 6. Git 与范围

- 公共协议、core runtime、ResourceManager、PluginHost、热重载或目录边界变化，提交前必须检查 diff 范围。
- 不覆盖用户或其他 Agent 的已有改动。
- 历史 version report 保留历史事实，不要求随当前架构改写。
