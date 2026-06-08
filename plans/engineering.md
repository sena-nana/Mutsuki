# MutsukiBot 工程实现规则

本文件描述当前工作树的事实：根目录是 **Rust-first Agent runtime
framework**。早期 Python 框架实现已经移动到
`python/legacy-mutsukibot/`，只作为 legacy / reference material 保留。

## 1. 技术栈

- **Rust 2024 + Cargo workspace**：根级主框架。
- **serde / serde_json**：跨 host 与可持久化 snapshot 的纯协议序列化。
- **thiserror**：结构化 runtime failure wrapper。
- **Python 3.13 + uv**：仅用于 `python/legacy-mutsukibot/` 的旧实现与参考测试；
  不再是根级 runtime 依赖。

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
    legacy-mutsukibot/          # 旧 Python 框架、扩展、测试、docs、examples
```

## 3. Rust Crate 边界

- `mutsuki-runtime-contracts` 只定义可序列化纯数据结构：
  `AgentSpec`、`Envelope`、`ScopeRuleSpec`、`OperationSnapshot`、
  `SourceSnapshot`、`TraceSpan`、`RuntimeError`、`RefDescriptor`、
  `LeaseToken`、`ResourceRecord`。
- `mutsuki-runtime-core` 实现 runtime mechanics：
  Agent lifecycle、inbox tick、ScopeRule 路由、source 注册校验、Operation
  metadata registry、backend key 调用、trace bookkeeping、`ResourceGate`
  租约治理。
- `mutsuki-runtime-host` 提供 native Rust backend / host helper。它可以注册
  Source 与 Operation，驱动 `AgentRuntime` 跑通最小 Agent loop，不依赖
  Python PluginHost。

## 4. Backend 边界

Runtime 通过 trait 与上层能力宿主通信：

- `StrategyBackend`：`on_awake` / `on_input` / `next_step` / `on_stop`。
- `OperationBackend`：`list_operations` / `list_sources` / `invoke` /
  `operation_status`。
- `ResourceBackend`：`register_resource` / `acquire_resource` /
  `release_resource` / `list_records`。

Backend 可以是 native Rust host，也可以是后续 Python sidecar adapter。Rust
runtime 只保存可序列化 snapshot 与 handler key，不保存 callable、socket、SDK
client、真实 `Handle[T]` 或领域对象。

## 5. 横切公约

- Agent 是一等运行时实体；生命周期状态由 `AgentRuntime` 维护。
- 核心不内置业务概念；LLM、记忆、情感、睡眠、IM、MCP、ChatCompletion 等只能在
  host / reference plugin / legacy Python 层表达。
- Operation 是工具、命令和跨能力调用的统一 runtime 概念；Rust 侧只持有
  `OperationSnapshot` 与 `OperationHandlerKey`。
- Source 必须先注册；未注册 Source 的 envelope publish 必须 fail-loud 为
  `source.unregistered`。
- 决定性时间、ID、随机源必须由 runtime / host 注入；`ResourceGate` 不使用全局
  UUID 源，租约 token 由注入式 ID source 生成。
- 错误必须使用 `RuntimeError` / `RuntimeFailure` 结构化表达，不吞异常式返回默认值。
- Trace span 必须保留 `trace_id` / `span_id` / `parent_span_id`，用于证明 Agent
  input、strategy、operation、resource 的因果链。
- Rust crates 不得出现 Yume、latent、tensor、gpu、Lilia、Codex、OneBot、MCP 等
  领域或产品专用执行分支。

## 6. 验证

根级必跑：

```powershell
cargo test
```

改动 legacy Python 时，从 `python/legacy-mutsukibot` 目录运行对应 Python 验证。
根级成功说明 Rust framework 可构建和通过 Rust contract / runtime / host 测试；
不得用 legacy Python 测试代替 Rust 主框架验证。
