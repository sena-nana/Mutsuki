# MutsukiBot 概念边界图谱

本文描述当前 Rust-first runtime 的核心概念。旧 Python 插件框架概念仅保留在
`python/reference-mutsukibot/` 中作为参考与迁移材料。

## 1. Agent

Agent 是运行时主体，拥有 identity、phase、priority、participation、accepts 和
inbox。Agent 不等于会话、LLM 调用、Operation handler 或 host。

Runtime 内化：

- lifecycle phase
- accepts 路由边界
- inbox
- owner 候选排序所需 metadata

Host 内化：

- 行为策略
- 具体工具与外部副作用
- 领域状态

## 2. Backend / Host

Backend 是 runtime 外部的能力宿主。它实现 `StrategyBackend`、`OperationBackend`
和可选 `ResourceBackend`，为 runtime 提供策略推进、Source / Operation snapshot 和
handler key 调用。

反模式：

- runtime 保存 host callable
- runtime import host 实现模块
- host 修改 Agent identity、participation 或 accepts

## 3. Envelope / Source / ScopeRule

Envelope 是输入载体。Source 描述输入来源。ScopeRuleSpec 是纯数据路由谓词。

硬规则：

- Source 必须先注册。
- 未注册 Source 返回 `source.unregistered`。
- Agent accepts 为空时不接收 envelope。
- ScopeRuleSpec 不执行 I/O，不读取外部可变状态。

## 4. Operation

Operation 是工具、命令、人类入口和跨能力调用的统一 runtime 概念。Rust runtime
只保存 `OperationSnapshot` 与 `OperationHandlerKey`。真实 handler 属于 backend。

调用路径：

```text
AgentRuntime.invoke_operation
  -> operation registry lookup
  -> snapshot status check
  -> OperationBackend.invoke(agent_id, key, payload)
```

反模式：

- runtime 直接调用 host 函数
- stale key fallback 到新 handler
- Operation 缺失时返回默认值

## 5. Resource

ResourceGate 管理资源治理事实，不持有真实对象：

- RefDescriptor
- owner
- LeaseToken
- lease_count

真实对象和 finalizer 属于 host。跨边界只传 descriptor、ref_id、lease token 和
可序列化 payload。

## 6. Trace

Trace 是 runtime 事实的一部分，用于解释 lifecycle、input、strategy、operation 和
resource 因果链。Trace sink 可以由 host 提供，但 sink 失败不应反向决定 Agent 行为。

## 7. Domain Boundary

Rust crates 只承载通用运行机制，不承载 Yume、mind-sim、IM、MCP、LLM、工程工具、
Codex、OneBot 等业务语义。需要这些能力时，由 host / sidecar / Python reference
通过 Source、Operation、Resource lease 组合表达。

## 8. 放置规则

- 定义“谁在运行” -> Agent / AgentRuntime
- 定义“输入从哪来” -> SourceRef / SourceSnapshot
- 定义“谁能收到输入” -> ScopeRuleSpec / accepts
- 定义“能调用什么” -> OperationSnapshot
- 定义“怎么执行” -> Backend / Host
- 定义“资源治理事实” -> ResourceGate
- 定义“真实资源对象” -> Host
- 定义“观测证据” -> TraceSpan
