# MutsukiBot 路线图

本文件回答：当前仓库目标、完成门槛、后续方向。当前工作树以 **Rust framework
完整可使用** 为主目标；早期 Python 框架代码已移动到
`python/reference-mutsukibot/`，作为旧 Python 实现的参考与迁移层，不再是根级主链。

## 当前边界：Rust-first Agent Runtime Kernel

根级 workspace 由三个 crate 组成：

- `crates/mutsuki-runtime-contracts`：纯协议与序列化结构。
- `crates/mutsuki-runtime-core`：运行时内核，负责 Agent lifecycle、routing、
  tick、Operation / Source registry、trace、ResourceGate。
- `crates/mutsuki-runtime-host`：native Rust host helper，用于不依赖 Python
  PluginHost 的可运行 smoke 和集成入口。

当前目标不是“把旧 Python Core 包一层 Rust 壳”，而是让 Rust runtime 本身具备
可直接嵌入应用的完整骨架：上层 host 只提供策略和能力实现，runtime 持有运行时
状态、调度、路由、source/operation metadata、资源租约和 trace 事实。

## 已完成基线

- Rust contracts 覆盖 `AgentSpec`、`Envelope`、`ScopeRuleSpec`、
  `OperationDescriptor`、`SourceDescriptor`、`OperationSnapshot`、
  `SourceSnapshot`、`StrategyResult`、`RuntimeError`、`TraceSpan`、
  `RefDescriptor`、`LeaseToken`、`ResourceRecord`。
- Rust core 覆盖：
  - `spawn -> awake -> sleep -> stop` 生命周期。
  - `publish` 路由与 `Agent.accepts` 显式匹配。
  - Source registry 校验；未注册 source fail-loud 为 `source.unregistered`。
  - Operation metadata registry 与 backend key 间接调用。
  - 启动事务：`on_awake` 或 registry refresh 失败时不提交 `awake`。
  - `ResourceGate` 管理 descriptor、owner、lease token、lease count。
  - 租约 token 由注入式 ID source 生成，不使用全局 UUID。
  - trace span 记录 Agent input / strategy / operation 等关键运行点。
- Rust host 覆盖：
  - native in-memory Source / Operation backend。
  - 无 Python 情况下跑通 Agent start、publish、tick、invoke、stop。
- Python reference：
  - 旧 `mutsukibot`、`mutsukibot_ext`、Python tests、docs、examples、`pyproject.toml`
    与 `uv.lock` 已移动到 `python/reference-mutsukibot/`。

## 当前完成门槛

Rust framework 被视为当前目标完成，必须同时满足：

- `cargo test` 在根目录通过。
- Rust runtime 可在不装载 Python 的情况下由 native host 跑通最小 Agent loop。
- Source 未注册、operation 缺失、backend generation mismatch、资源 token mismatch
  都以结构化错误失败。
- Resource acquire / release 计数正确，lease token 由 runtime/host ID source 生成。
- Trace 至少能证明 input -> strategy 与 operation 错误链路的父子关系。
- Rust crates 中不出现 Yume、latent、tensor、gpu、Lilia、Codex、OneBot、MCP 等
  领域或产品专用执行分支。
- 根级 README / plans 不再把 Python Core 描述为当前主运行时。

## 下一步

### R5：Native Framework Hardening

- 增加 runtime event stream，让 host 能订阅 lifecycle / routing / resource / trace
  事件，而不是直接读内部结构。
- 为 `ResourceGate` 增加容量治理和 `capability.exhausted` 门控。
- 将 trace closure helper 移植到 Rust 测试工具，覆盖重复 span、父链缺失、时间区间。
- 引入可替换 election policy trait，但 policy 只能排序已通过 lifecycle + accepts
  过滤的候选。

### Optional：Python Reference Adapter

- 如果后续需要继续承载 Python 插件生态，在 `python/reference-mutsukibot` 内维护
  sidecar / adapter，不得让根级 Rust crates 依赖 Python。
- Python 侧只能通过纯协议与 backend key 和 Rust runtime 交互，不得跨边界传
  callable、socket、SDK client、真实 `Handle[T]` 或领域对象。

## 反向论证（红线）

出现以下情况应修 runtime 契约，而不是把业务语义塞回 core：

- Rust runtime 需要理解 latent、KV cache、LLM provider、IM wire shape 或产品工具。
- 为了跨边界调用而序列化真实 `Handle[T]`。
- Source 未声明也能路由，或 backend key 过期后自动 fallback 到新 handler。
- trace 断链但没有结构化错误解释。
- 为性能绕过 capability、permission、scope、source 或 trace 拦截链。

## Plan 同步规则

- 代码即事实，plans 是契约 + 决策。
- 公共契约、生命周期、backend trait、资源治理或目录边界变化必须同 PR 更新 plans。
- 历史版本报告保留历史上下文；当前事实以本文件和 `engineering.md` 为准。
