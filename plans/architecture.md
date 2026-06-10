# MutsukiBot 架构设计

MutsukiBot 当前根级实现是一个 **Rust-first Agent runtime framework**。它的目标是
为 Yume / mind-sim、工程 Agent 与传统 Bot 能力提供领域中立运行内核，但不把
任一业务语义写入 core。

早期 Python 框架实现已经移动到 `python/reference-mutsukibot/`。该目录是参考与
迁移层，不代表废弃内容，也不再定义根级 runtime 主链。

## 1. 项目方向

Rust runtime 负责运行机制：

- Agent lifecycle 与状态事实。
- Envelope 路由和 accepts 匹配。
- runtime 级插件启用 / 禁用状态，以及 Source / Operation metadata registry。
- Backend key 间接调用。
- Resource descriptor / lease / count 治理。
- Trace span 因果链 bookkeeping。
- Runtime event stream 与可替换 election policy。

Caller / Host / Backend 负责行为语义：

- Native Rust host 可以直接注册 Source / Operation 和 StrategyBackend。
- Python backend kit 可作为可选 backend / sidecar foundation 存在，但不得被 Rust
  crates 依赖。
- Python reference host 只作为旧实现参考与迁移材料存在。
- Yume、mind-sim、IM、LLM、MCP、ChatCompletion、工程工具等都属于 host /
  plugin / reference layer。
- Python 插件入口也可以作为 runtime caller，通过 runtime API 向 Rust kernel 发布事件
  或查询状态；它不因此成为第二个 runtime kernel。

## 2. 分层

```text
Caller
  -> Rust Runtime Kernel
  -> Capability Backend

Caller examples:
  Rust application / Python plugin entry / HTTP service / CLI / other project

Rust Runtime Kernel:
  crates/mutsuki-runtime-core
  crates/mutsuki-runtime-contracts

Capability Backend examples:
  crates/mutsuki-runtime-host (optional native helper)
  python/mutsuki-runtime-python (Python backend kit / sidecar foundation)
  remote service adapter

python/reference-mutsukibot
  -> optional reference / migration material only
```

依赖方向：

- `contracts` 不依赖 `core` 或 `host`。
- `core` 依赖 `contracts`，不依赖具体 host、Python、外部协议 SDK 或产品语义。
- `host` 依赖 `core + contracts`，提供可直接运行的 native helper。
- Python reference 层不被根级 Rust crates 依赖。
- Python backend kit 不被根级 Rust crates 依赖；它镜像 contracts 并承载 Python
  owned backend handler，并提供 stdio JSONL 进程边界；它不拥有 Rust runtime 状态事实。
- 未来远程 Rust runtime 应通过单独的 `runtime.*` control protocol 暴露给 caller；
  当前 `backend.*` JSONL 边界只表示 Rust runtime 调 capability backend。

## 3. Agent 一等公民

Agent 是运行时实体，而不是会话、消息、LLM 调用或命令 handler。Rust
`AgentRuntime` 持有 Agent 的：

- `agent_id`
- lifecycle phase
- priority / participation / accepts
- inbox
- 插件启用状态下接入的 Operation / Source snapshots
- trace 与资源治理关联事实

Agent 行为由 backend 推进：

- `StrategyBackend.on_awake`
- `StrategyBackend.on_input`
- `StrategyBackend.next_step`
- `StrategyBackend.on_stop`

Backend 不能修改 Agent identity、owner、participation 或 accepts；这些是 runtime
边界事实。

## 4. Operation 与 Source

插件接入是 runtime 级事实。Caller 可以实时启用 / 禁用插件；只有 enabled 插件
提供的 Source / Operation 会进入 runtime registry。Rust core 只保存
`PluginSnapshot`、`PluginAccessState`、`OperationSnapshot` 和 `SourceSnapshot`，
不扫描、安装或加载插件。

Operation 是工具、命令和跨能力调用的统一 runtime 概念。Rust runtime 只保存：

- `OperationDescriptor`
- `OperationStatus`
- `OperationHandlerKey`

真实 handler 属于 backend。runtime 调用 Operation 时必须通过 backend key，不保存
callable 或外部资源对象。

Source 是 Envelope 来源的显式声明。`publish(envelope)` 必须先验证
`envelope.source.source_id` 已注册；未注册 source 结构化失败为
`source.unregistered`。

## 5. Resource 与 By-Ref

Rust `ResourceGate` 管理资源治理事实：

- `RefDescriptor`
- owner
- `LeaseToken`
- lease count
- `ref_id` / resource `kind` 维度的租约容量策略

真实对象、finalizer、socket、SDK client、tensor、KV cache 等不进入 Rust core。
跨 host 边界只能传 `ref_id`、descriptor、lease token 和可序列化 payload。

## 6. Trace

Runtime 产生结构化 `TraceSpan`，用于证明 lifecycle、input、strategy、operation
和 resource 的因果链。Trace sink 可以由 host 或 observability layer 提供，但 trace
失败不得改变主链路行为，除非 trace 结构本身破坏 runtime 契约。

Rust core 提供 trace closure helper，用于测试或 host 诊断重复 span、父链缺失、
父子 trace 不一致和无效时间区间。

## 7. Runtime Event 与 Election

Runtime event stream 以 `RuntimeEvent` 纯协议暴露 lifecycle、routing、operation、
resource、trace 和 backend 事实。事件使用确定性 sequence，不要求 wall-clock time。
Trace 事实仍以 `TraceSpan` 为事实源，并同步投影为 `trace.span` event；resource
事实只在 `AgentRuntime` 拥有的 `ResourceGate` 内暂存为 event draft，随后由 runtime
统一分配 sequence。Standalone `ResourceGate` 不产出可观察事件。

默认 Agent election 仍按 priority 降序、agent_id 升序；可替换 election policy 只能
处理已通过 source 注册、awake、accepts 与 participation 过滤的候选，不得绕过 runtime
边界事实。

## 8. Python Reference

`python/reference-mutsukibot/` 保留旧 Python framework、extension、tests、docs 与
examples，方便迁移和对照。它不再是当前主实现，但不是废弃目录。

## 9. Python Backend Kit

`python/mutsuki-runtime-python/` 是当前新版 Python 端结构。它提供：

- 与 Rust contracts 对齐的 Python dataclass wire shape。
- `StrategyBackend`、`OperationBackend`、`ResourceBackend` 协议。
- 进程内 `PythonBackendHost`，用于注册 Python-owned operation handler、source
  snapshot、plugin snapshot 和 strategy hook。
- `PythonResourceBackend`，只保存 descriptor、lease token 和 lease count。
- `StdioJsonlBackendServer`，通过 JSONL request/response 暴露显式进程边界。

该包不复刻 AgentRuntime，不实现 routing / lifecycle / trace 的 Rust 事实源，也不
依赖旧 `mutsukibot` core。Python 侧长期只保管插件元信息、插件行为、真实资源对象、
外部协议接入和 Python 异常到 `RuntimeError` 的映射。stdio JSONL 是当前显式 backend 进程边界；
后续若增加 HTTP 或长期 sidecar supervisor，只能复用这些纯协议对象与 backend key。

若未来恢复 Python PluginHost：

- 可以作为 runtime caller，也可以作为 backend / sidecar，但不能拥有 runtime 事实源。
- 只能通过纯协议与 Rust runtime 交互；状态推进进入 Rust runtime 的 turn / queue 边界。
- 不得让 Rust core 保存 Python callable、真实 `Handle[T]`、socket 或 SDK client。
- 旧 generation key 必须 fail-loud，不允许 fallback 到新 handler。

## 10. Domain Neutrality

Rust crates 不得包含 Yume、latent、tensor、gpu、Lilia、Codex、OneBot、MCP、
ChatCompletion 等领域或产品专用执行分支。需要这些能力时，在 host / plugin /
Python backend kit / reference 层通过 contracts、Operation、Source、Resource lease
组合表达。

Yume / Lilia 的人格、记忆、情绪、LLM provider、IM 接入和工程工具能力应作为
Caller 或 Capability Backend 组合到 runtime 外侧；Rust core 只保留领域中立调度事实。
