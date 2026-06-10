# MutsukiBot 工程实现规则

本文件描述当前工作树的事实：根目录是 **Rust-first Agent runtime
framework**。早期 Python 框架实现已经移动到
`python/reference-mutsukibot/`，作为旧 Python 实现的参考与迁移层保留。

## 1. 技术栈

- **Rust 2024 + Cargo workspace**：根级主框架。
- **serde / serde_json**：跨 host 与可持久化 snapshot 的纯协议序列化。
- **thiserror**：结构化 runtime failure wrapper。
- **Python 3.13 + uv**：用于 `python/reference-mutsukibot/` 的旧实现与参考测试，
  以及 `python/mutsuki-runtime-python/` 的新版 backend kit；二者都不是根级 Rust
  runtime 依赖。

根级 Rust crates 禁止依赖 Python、PyO3、动态插件系统、外部 IM 协议 SDK、LLM
provider、Yume / mind-sim / Lilia 产品语义。

## 2. 目录结构

```text
MutsukiBot/
  AGENTS.md
  README.md
  Cargo.toml
  Cargo.lock
  crates/
    mutsuki-runtime-contracts/  # 纯协议对象与 ScopeRuleSpec 匹配
    mutsuki-runtime-core/       # AgentRuntime / ResourceGate / backend traits
    mutsuki-runtime-host/       # native in-memory host helper / smoke tests
  plans/
    roadmap.md
    architecture.md
    engineering.md
    contracts.md
    rust-python-runtime-boundary.md
    version-reports/
  python/
    mutsuki-runtime-python/    # 新版 Python backend kit：contracts mirror / host / resource / tests
    reference-mutsukibot/       # 旧 Python 框架、扩展、测试、docs、examples 的参考与迁移层
```

## 3. Rust Crate 边界

- `mutsuki-runtime-contracts` 只定义可序列化纯数据结构：
  `AgentSpec`、`Envelope`、`ScopeRuleSpec`、`OperationSnapshot`、
  `SourceSnapshot`、`PluginSnapshot`、`PluginAccessState`、`AgentSnapshot`、
  `TraceSpan`、`RuntimeEvent`、`RuntimeError`、`RefDescriptor`、`LeaseToken`、
  `ResourceRecord`。
- `mutsuki-runtime-core` 实现 runtime mechanics：
  Agent lifecycle、inbox tick、ScopeRule 路由、runtime 级插件启用 / 禁用、
  source 注册校验、Operation metadata registry、backend key 调用、trace bookkeeping、`ResourceGate`
  租约治理、runtime event stream、trace closure helper 和 election policy。
- `mutsuki-runtime-host` 提供 native Rust backend / host helper。它可以注册
  Source 与 Operation，驱动 `AgentRuntime` 跑通最小 Agent loop；它也提供泛型
  stdio JSONL backend adapter，但不依赖 Python PluginHost。
- `python/mutsuki-runtime-python` 提供新版 Python backend kit。它镜像 Rust
  contracts，保存 Python-owned handler，并通过 backend key 暴露 operation/source
  snapshot，且提供 stdio JSONL 进程边界；它不拥有 Rust runtime 状态事实。

## 4. Backend 边界

Runtime 通过 trait 与上层能力宿主通信：

- `StrategyBackend`：`on_awake` / `on_input` / `next_step` / `on_stop`。
- `OperationBackend`：`list_operations` / `list_sources` / `invoke` /
  `operation_status`，并通过 `list_plugins` 暴露插件元信息。
- `ResourceBackend`：`register_resource` / `acquire_resource` /
  `release_resource` / `list_records`。

Backend 可以是 native Rust host，也可以由 `python/mutsuki-runtime-python` 提供的
Python backend kit / 后续 sidecar adapter 承载。Rust runtime 只保存可序列化
snapshot 与 handler key，不保存 callable、socket、SDK client、真实 `Handle[T]` 或
领域对象。

## 5. 横切公约

- Agent 是一等运行时实体；生命周期状态由 `AgentRuntime` 维护。
- 核心不内置业务概念；LLM、记忆、情感、睡眠、IM、MCP、ChatCompletion 等只能在
  host / reference plugin / Python backend kit / Python reference 层表达。
- Operation 是工具、命令和跨能力调用的统一 runtime 概念；Rust 侧只持有
  `OperationSnapshot` 与 `OperationHandlerKey`。
- 插件接入是 runtime 级启用 / 禁用状态；Rust core 只保存插件元信息和启用状态，
  不负责扫描、安装或加载插件。
- Source 必须先注册；未注册 Source 的 envelope publish 必须 fail-loud 为
  `source.unregistered`。
- 决定性时间、ID、随机源必须由 runtime / host 注入；`ResourceGate` 不使用全局
  UUID 源，租约 token 由注入式 ID source 生成。
- 错误必须使用 `RuntimeError` / `RuntimeFailure` 结构化表达，不吞异常式返回默认值。
- Trace span 必须保留 `trace_id` / `span_id` / `parent_span_id`，用于证明 Agent
  input、strategy、operation、resource 的因果链。
- Runtime event stream 必须只记录纯协议事件，不携带真实资源对象或 callable；事件
  `sequence` 由 runtime 全局分配，drain 后也不能回退或复用。Trace span 以
  `TraceSpan` 为事实源，并同步投影为 `trace.span` event。Resource events 只属于
  `AgentRuntime` 事件流；runtime-owned `ResourceGate` 可暂存内部 event draft，standalone
  `ResourceGate` 不维护可观察 event stream，也不收集 draft。
- Resource quota 耗尽必须 fail-loud 为 `capability.exhausted`，不得静默创建租约；
  `kind` quota 按同 kind 全部活跃 lease 总量计算。
- Election policy 只能排序已过滤候选，不得绕过 source / lifecycle / accepts。
- Rust crates 不得出现 Yume、latent、tensor、gpu、Lilia、Codex、OneBot、MCP 等
  领域或产品专用执行分支。

## 6. 验证

根级必跑：

```powershell
cargo test
```

改动 Python backend kit 时，从 `python/mutsuki-runtime-python` 目录运行：

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

改动 Python reference 层时，从 `python/reference-mutsukibot` 目录运行对应 Python 验证。
根级成功说明 Rust framework 可构建和通过 Rust contract / runtime / host 测试；
不得用 Python reference 层测试代替 Rust 主框架验证。
