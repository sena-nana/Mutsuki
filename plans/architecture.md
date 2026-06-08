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
- Source / Operation metadata registry。
- Backend key 间接调用。
- Resource descriptor / lease / count 治理。
- Trace span 因果链 bookkeeping。

Host 负责行为语义：

- Native Rust host 可以直接注册 Source / Operation 和 StrategyBackend。
- Python reference host 可作为可选 sidecar / adapter 存在，但不得被 Rust crates
  依赖。
- Yume、mind-sim、IM、LLM、MCP、ChatCompletion、工程工具等都属于 host /
  plugin / reference layer。

## 2. 分层

```text
Application / Host
  -> mutsuki-runtime-host (optional native helper)
  -> mutsuki-runtime-core
  -> mutsuki-runtime-contracts

python/reference-mutsukibot
  -> optional reference / migration material only
```

依赖方向：

- `contracts` 不依赖 `core` 或 `host`。
- `core` 依赖 `contracts`，不依赖具体 host、Python、外部协议 SDK 或产品语义。
- `host` 依赖 `core + contracts`，提供可直接运行的 native helper。
- Python reference 层不被根级 Rust crates 依赖。

## 3. Agent 一等公民

Agent 是运行时实体，而不是会话、消息、LLM 调用或命令 handler。Rust
`AgentRuntime` 持有 Agent 的：

- `agent_id`
- lifecycle phase
- priority / participation / accepts
- inbox
- Operation / Source snapshots
- trace 与资源治理关联事实

Agent 行为由 backend 推进：

- `StrategyBackend.on_awake`
- `StrategyBackend.on_input`
- `StrategyBackend.next_step`
- `StrategyBackend.on_stop`

Backend 不能修改 Agent identity、owner、participation 或 accepts；这些是 runtime
边界事实。

## 4. Operation 与 Source

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

真实对象、finalizer、socket、SDK client、tensor、KV cache 等不进入 Rust core。
跨 host 边界只能传 `ref_id`、descriptor、lease token 和可序列化 payload。

## 6. Trace

Runtime 产生结构化 `TraceSpan`，用于证明 lifecycle、input、strategy、operation
和 resource 的因果链。Trace sink 可以由 host 或 observability layer 提供，但 trace
失败不得改变主链路行为，除非 trace 结构本身破坏 runtime 契约。

## 7. Python Reference

`python/reference-mutsukibot/` 保留旧 Python framework、extension、tests、docs 与
examples，方便迁移和对照。它不再是当前主实现，但不是废弃目录。

若未来恢复 Python PluginHost：

- 只能作为 backend / sidecar。
- 只能通过纯协议与 Rust runtime 交互。
- 不得让 Rust core 保存 Python callable、真实 `Handle[T]`、socket 或 SDK client。
- 旧 generation key 必须 fail-loud，不允许 fallback 到新 handler。

## 8. Domain Neutrality

Rust crates 不得包含 Yume、latent、tensor、gpu、Lilia、Codex、OneBot、MCP、
ChatCompletion 等领域或产品专用执行分支。需要这些能力时，在 host / plugin /
Python reference 层通过 contracts、Operation、Source、Resource lease 组合表达。
