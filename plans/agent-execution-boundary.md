# Agent 执行边界设计：固定主体，可变策略

本文件记录 MutsukiBot 在承载普通编程 Agent（类似 Claude / Codex 的固定
step loop）时的架构边界。目标是复用 MutsukiBot 的插件生态，同时不把 Agent
抽象降级成一个可被 runner 插件随意替换的空壳。

## 1. 背景与问题

MutsukiBot 最初面向 Yume / mind-sim 这类高度扩展的 Agent 架构设计：Agent
是一等运行时实体，插件通过契约、服务、Operation、Source 和 ResourceHost
组合能力。

普通编程 Agent 的运行流程更固定，通常是：

```text
接收用户输入
  -> 规划下一步
  -> 调用工具或子 Agent
  -> 观察结果
  -> 更新状态
  -> 回复 / 继续 / 暂停
```

如果直接让这种 Agent 完全套用现有 Bot 型 `publish -> inbox -> scheduler`
流程，会有两个风险：

- 外部输入被多个 Agent 或插件广播消费，导致主流程不确定。
- 为了快速支持编程 Agent，可能在 core / scheduler / dispatcher 中加入协议
  补丁，最终让核心承担 Claude / Codex / MCP / shell 等特化语义。

但反过来，如果把 step loop 做成完全可替换的 runner，也会产生另一个问题：
Agent 的核心逻辑都被替换后，Agent 还剩下什么？若 Agent 只剩插件容器，
它就不再值得作为单独层级存在。

本设计的答案是：**Agent 是固定主体，ExecutionStrategy 只是受约束的行为策略**。

## 2. 核心立场

Agent 不是插件，也不是 runner 的容器。Agent 是稳定主体，必须保留以下不可
替换职责：

- **身份与归属**：`agent_id`、owner、session ownership、副作用责任属于 Agent。
- **状态边界**：上下文、inbox/outbox、长期状态、trace root、资源租约归属属于 Agent。
- **生命周期**：`spawn / awake / sleep / stop` 由 Agent 层统一管理。
- **调度纪律**：并发、取消、暂停、恢复、新输入接纳策略由 Agent 裁决。
- **能力裁决**：插件声明 capability，Agent 决定当前上下文是否允许使用。
- **对外承诺**：外部世界看到的是某个 Agent 在行动，而不是某个 runner 插件。

因此，核心不引入“完全可替换 AgentRunLoop”。可变部分命名为
`ExecutionStrategy` 或 `AgentPolicy`，它只能在 Agent 授权边界内决定下一步
如何推进。

```text
Agent = 身份 / 状态 / 生命周期 / 权限 / 调度边界
ExecutionStrategy = Agent 内部的下一步推进策略
Plugins = Agent 可调用的能力、资源、服务与外部协议桥
```

## 3. ExecutionStrategy 边界

不同 Agent 形态的差异应体现为策略，而不是核心专用分支：

- Bot strategy：从消息中解析命令，调用 Operation，输出回执。
- Yume strategy：推进持续认知循环、处理 stimulus、生成 expression。
- Coding strategy：执行固定的 plan-act-observe-update step loop。

策略必须遵守以下约束：

- 不拥有或绕过 Agent 生命周期。
- 不改变 Agent 的 `agent_id`、owner、participation、accepts、permission。
- 不直接决定外部输入归属。
- 不直接 import 兄弟插件实现模块。
- 不绕过 `ctx.dispatch.invoke(...)` 调用工具。
- 不绕过 `ctx.dispatch.invoke_in_agent(...)` 调用其他 Agent。
- 不直接持有未挂入 `PluginScope` / `ResourceHost` 的 I/O 资源。

策略可以决定：

- 收到一个已归属给当前 Agent 的 envelope 后，下一步先调用哪个 Operation。
- 是否继续推进下一 step、暂停、请求用户输入或结束。
- 如何把工具观察结果折叠回 Agent 状态。
- 如何发出 trace / Decision，供 observability 与 replay 使用。

## 4. 多 Agent 输入归属

编程 Agent 的主输入不能默认广播给所有匹配 Agent。多 Agent 路由采用
“单主 + 只读旁路”模型：

- `primary_candidate`：可成为某个 source/session 的 owner，并推进主流程。
- `observer`：只能收到旁路副本，默认只读，不产生外部副作用。
- `explicit_helper`：不自动接收外部输入，只能被主 Agent 显式调用。

外部输入进入后，先由通用路由层根据 `Agent.accepts`、生命周期与 election policy
选出 owner。选中后建立稳定 binding：

```text
(source_id, session_id | channel_id | workspace_id) -> agent_id
```

后续同一输入流继续投递给同一个 owner Agent。旁路 Agent 可接收只读副本，用于
记忆、审计、索引、评估或监控，但默认不能回复用户或修改外部状态。

Agent 之间的协作分两类：

- 同步请求：主 Agent 使用 `dispatch.invoke_in_agent(target_agent_id, op_id, payload)`。
- 旁路观察：使用现有 `publish` / trace 机制广播给 observer。

## 5. 契约草案

设计层建议新增以下契约；字段名可在实现阶段根据现有代码风格调整。

```text
AgentParticipation:
  primary_candidate
  observer
  explicit_helper
```

```text
AgentProfile:
  profile_id: str
  participation: AgentParticipation
  accepts: tuple[ScopeRule, ...]
  strategy_id: str
  side_effect_policy: SideEffectPolicy
```

```text
ExecutionStrategy:
  strategy_id: str
  supported_profiles: tuple[str, ...]
  on_awake(ctx) -> Awaitable[None]
  on_input(ctx, envelope) -> Awaitable[StrategyResult]
  on_stop(ctx) -> Awaitable[None]
  next_step(ctx) -> Awaitable[StrategyResult]  # 可选，用于固定 step loop
```

```text
StrategyResult:
  status: continue | wait_input | completed | failed
  decision: Decision | None
  emitted: tuple[Envelope, ...]
  error: Error | None
```

`ExecutionStrategy` 不是 Agent 身份主体。它由 Agent 装配和约束，不能绕过 Agent
的 ownership、permission、capability 和 lifecycle。

## 6. 与现有机制的关系

- `Agent.accepts` 仍是外部输入可达性的第一道声明；空 tuple 表示不接收 envelope。
- `AgentRegistry` 仍负责生命周期过滤与候选排序；策略插件只能排序候选，不能绕过过滤。
- `Dispatcher.invoke` 仍是工具 / 命令 / 跨插件 RPC 的唯一调用入口。
- `Dispatcher.publish` 继续保留广播语义，主要用于旁路观察和事件 fan-out。
- `PluginScope` / `ResourceHost` 继续管理资源生命周期；策略不得裸持 socket、SDK client 或连接对象。
- Permission 与 Capability 仍正交：Capability 声明能做什么，Agent/Profile 决定当前上下文是否允许做。

## 7. 第一阶段范围

第一阶段只做设计与契约验证，不实现完整 Codex。

范围内：

- 固化 Agent 是主体、ExecutionStrategy 是策略的边界。
- 设计 `AgentParticipation` / `AgentProfile` / `ExecutionStrategy` 契约。
- 设计单主 + 只读旁路路由语义。
- 为后续最小 coding strategy 冒烟测试定义验收标准。

范围外：

- 不实现真实 shell / browser / git / LLM 工具集。
- 不引入 Claude / Codex / coding 专用概念到 core。
- 不删除现有 Bot scheduler 或 `publish` 广播路径。
- 不改变现有插件直接通过 Operation / Source 暴露能力的模式。

## 8. 验收标准

后续实现本设计时，至少需要覆盖以下场景：

- 替换 ExecutionStrategy 后，Agent 的 `agent_id`、owner、生命周期、trace root 不变。
- observer Agent 即使匹配输入，也不会成为 owner。
- 同一 session 的后续输入稳定路由到同一个 primary Agent。
- strategy 调工具必须走 dispatcher，不能直接调用插件实现。
- side-effect Operation 在 observer Agent 上被拒绝，除非主 Agent 显式授权。
- explicit helper 不自动接收外部输入，只能被 `invoke_in_agent` 调用。
- strategy 卸载后不残留未释放资源、路由 binding 或未注销回调。
- 现有 Bot scheduler 行为保持兼容。

## 9. 反向判定

若后续实现出现以下迹象，应停止并修正设计：

- core 出现 `Codex`、`Claude`、`coding` 等产品特化字段或分支。
- ExecutionStrategy 可以修改 Agent 身份、owner、participation 或 accepts。
- strategy 直接 import 工具插件实现，绕过 Operation。
- observer 能在默认情况下修改外部状态。
- 多个 primary Agent 同时消费同一用户输入并产生主回复。
- 为了快速适配某个协议，在 dispatcher / scheduler 中加入外部协议专用补丁。

本设计的目标不是削弱 MutsukiBot 的 Agent 抽象，而是让 Agent 的主体性更清楚：
插件扩展能力，策略推进行为，Agent 承担身份、状态、生命周期与责任。
