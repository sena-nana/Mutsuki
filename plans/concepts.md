# MutsukiBot 概念边界图谱

本文件回答：MutsukiBot 中每个核心概念是什么、包含什么、哪些必须由框架内化、
哪些可以由插件或协议替换，以及它如何与其他概念协同。

阅读顺序建议：先读本文建立概念地图，再读 [architecture.md](architecture.md)、
[contracts.md](contracts.md)、[engineering.md](engineering.md) 和
[agent-execution-boundary.md](agent-execution-boundary.md)。

## 1. 总览

MutsukiBot 的核心分工：

```text
Agent
  拥有身份、状态、生命周期、权限裁决与调度边界

ExecutionStrategy / AgentPolicy
  在 Agent 授权边界内决定下一步如何推进

Plugin
  提供可装卸能力、服务、Operation、Source、协议桥和领域模块

Operation
  统一命令、工具调用与跨插件 RPC

Source / Envelope / ScopeRule
  描述外部或内部输入从哪里来、是什么、应路由给谁

Dispatcher
  执行 Operation 调用与 Envelope 路由

ResourceHost / Handle / RefPayload
  管理非序列化资源、引用生命周期与跨插件按引用传递

Runtime
  提供时间、ID、随机源、并发与同步点治理

Observability
  旁路记录 trace、audit、metrics，不进入主链路依赖
```

判断一个新机制放在哪里，先问三件事：

1. 它是否定义“谁在行动”？是则属于 Agent。
2. 它是否定义“下一步怎么做”？是则属于 ExecutionStrategy / AgentPolicy。
3. 它是否提供“能做什么”？是则属于 Plugin / Operation / Service / Resource。

## 2. Agent

### 含义

Agent 是一等运行时主体，不是会话、不是 LLM 调用、不是插件容器。外部世界看到
的是某个 Agent 在行动；trace、权限、状态、资源租约与副作用责任都应归属到
Agent。

### 包含内容

- `agent_id` 与 owner。
- 生命周期：`spawn / awake / sleep / stop`。
- 运行期上下文：clock、id_gen、rng、services、scope、dispatch、trace_ctx。
- 状态边界：inbox/outbox、长期状态、资源租约归属、trace root。
- 调度边界：是否接收输入、并发限制、取消、暂停、恢复。
- 能力裁决：当前上下文是否允许调用某类 Operation 或资源。

### 必须内化

- 身份与所有权。
- 生命周期状态与转换。
- 与输入流的 ownership 关系。
- 外部副作用责任。
- 对 strategy 和 plugin 的授权边界。

### 可被替换或扩展

- Agent 使用哪种 ExecutionStrategy。
- Agent 装载哪些插件。
- Agent 接收哪些 ScopeRule。
- Agent 选举排序策略，但不能绕过生命周期与 `accepts` 过滤。
- Agent 内部状态的具体 schema，可由领域契约包定义。

### 如何协同

- Agent 通过 `PluginLoader` 装载插件。
- Agent 通过 `Dispatcher` 调用 Operation、发布 Envelope。
- Agent 通过 `AgentRegistry` 被发现、排序、选为目标。
- Agent 通过 ResourceHost / Handle 持有或借用资源。
- Agent 通过 Observability 输出 trace，而不是直接依赖 observability 实现。

### 反模式

- 把 Agent 当成普通插件。
- 把 Agent 等同于 LLM provider 或单次对话 session。
- 让 ExecutionStrategy 修改 Agent 身份、owner、participation 或 accepts。
- 让多个 primary Agent 同时消费同一用户输入并产生主回复。

## 3. ExecutionStrategy / AgentPolicy

### 含义

ExecutionStrategy 是 Agent 内部的行为策略，用于决定“下一步如何推进”。它不是
Agent 的身份主体，也不拥有生命周期。

### 包含内容

- 处理已归属给当前 Agent 的输入。
- 决定下一步调用哪个 Operation 或 helper Agent。
- 决定是否继续、暂停、等待用户、完成或失败。
- 把观察结果折叠回 Agent 状态。
- 输出 Decision / trace 供观测与回放。

### 必须内化

不应由 strategy 内化 Agent 身份、输入归属、权限边界、资源生命周期。strategy
只应内化“推进算法”。

### 可被替换或扩展

- Bot strategy：消息命令解析。
- Yume strategy：持续认知循环。
- Coding strategy：plan-act-observe-update 固定 step loop。
- 领域策略：例如 sleep、evaluation、multi-step planning。

### 如何协同

- 由 Agent 调用 strategy，而不是 strategy 接管 Agent。
- 调工具必须走 `ctx.dispatch.invoke(...)`。
- 调其他 Agent 必须走 `ctx.dispatch.invoke_in_agent(...)`。
- 借用资源必须走 ResourceHost / Handle。
- 发出的事件必须可被 trace 串联。

### 反模式

- strategy 直接 import 工具插件实现。
- strategy 直接持有 socket、SDK client 或连接对象。
- strategy 绕过 Agent 生命周期启动自己的长期主循环。
- strategy 决定外部输入归属或重写 Agent 权限。

## 4. Plugin

### 含义

Plugin 是可装载、可卸载的能力单元。它可以提供命令、Operation、Source、Service、
协议桥、领域模块或策略实现，但不应成为 Agent 主体。

### 包含内容

- Manifest：id、version、contracts、capabilities、config schema。
- 依赖声明：requires_plugins、requires_services、requires_operations、requires_sources。
- 提供声明：provides_services、provides_operations、provides_sources。
- 装载 / 卸载入口：on_load、on_unload。
- 可选 envelope 消费：consumes、on_envelope。
- 通过 PluginScope 注册的副作用与资源。

### 必须内化

- 配置 schema。
- 自身能力与依赖声明。
- 自身创建的资源如何释放。
- 对外暴露的 Operation / Source / Service 契约。

### 可被替换或扩展

- 外部协议桥：OneBot、MCP、ChatCompletion、文件系统、浏览器等。
- 领域能力：记忆、情感、睡眠、LLM、Yume kernel。
- ExecutionStrategy 实现。
- observability sink。

### 如何协同

- 通过 manifest 参与 DAG 拓扑加载。
- 通过 Dispatcher 注册 Operation / Source。
- 通过 ServiceContainer 提供或消费服务。
- 通过 PluginScope 把副作用绑定到卸载生命周期。
- 通过 contracts-only 包与其他插件共享类型。

### 反模式

- 插件之间直接 import 兄弟插件实现模块。
- 无 schema 的插件被装载。
- 插件持有未挂入 PluginScope / ResourceHost 的 I/O 资源。
- 插件未声明 capability 就调用对应能力。

## 5. Operation

### 含义

Operation 是统一的可调用入口。命令、人类触发操作、LLM tool、跨插件 RPC 和
外部 endpoint 调用在 MutsukiBot 内都应归一为 Operation。

### 包含内容

- `op_id`：Agent 内唯一。
- 参数 schema 与返回 schema。
- permission rule。
- requires_capabilities。
- tool manifest 信息。
- handler 与状态：active、unhealthy、unregistering。

### 必须内化

- 统一调用入口。
- capability / permission 检查顺序。
- trace span。
- handler 抛错后的结构化错误与 unhealthy 标记。
- PluginScope 自动反注册。

### 可被替换或扩展

- 具体 handler。
- 参数与返回契约。
- 权限规则。
- capability 命名空间。
- 是否同时暴露为 LLM tool。

### 如何协同

- Plugin 静态声明和运行时注册 Operation。
- Dispatcher inline await 调用 Operation。
- Agent / Strategy 通过 `ctx.dispatch.invoke(...)` 调用 Operation。
- PluginLoader 根据 requires_operations 建立 DAG 依赖。

### 反模式

- 为同一函数维护“命令”和“工具”两份实现。
- 直接调用插件方法绕过 Operation。
- 在 dispatcher 中加入某个外部协议的专用分支。
- Operation handler 吞异常返回默认值。

## 6. Source

### 含义

Source 描述 Envelope 从哪里来。Source 不是 handler，不执行业务逻辑，只声明
来源身份、类型和能力。

### 包含内容

- `source_id`：如 `qq:bot1`、`backend:default`。
- kind：IM 或 bridge / 领域插件注册的扩展类型。
- capabilities：该来源能产生的内容能力。
- description。

### 必须内化

- Source 必须先注册，才能发布来自该 source_id 的 Envelope。
- Source 与 Operation 的命名空间约定。
- PluginScope 卸载时反注册 Source。

### 可被替换或扩展

- 外部协议来源。
- 工具事件来源。
- 领域事件来源。
- SourceKindName 命名空间。

### 如何协同

- Plugin 注册 Source。
- Envelope.source 引用 Source。
- ScopeRule 使用 source_id、kind、capability 匹配路由。
- Dispatcher 校验 source 是否已注册。

### 反模式

- 未注册 Source 就发布 Envelope。
- 把 Source 做成独立 adapter 类型，绕开 Plugin。
- 在 Source 中塞入 handler 或业务状态。

## 7. Envelope

### 含义

Envelope 是通用入站 / 出站载体。IM Message、外部后端事件、领域 stimulus 都应是
Envelope 或 Envelope 的领域特化。Core 不内置应用后端事件 schema。

### 包含内容

- id 与 timestamp。
- source。
- payload_schema_id。
- capabilities_required。
- 领域 payload 或 message parts。

### 必须内化

- 路由所需的最小公共字段。
- 时间与 ID 由 runtime 注入。
- schema_id / schema_version 兼容约定。

### 可被替换或扩展

- payload schema。
- Message、bridge 自定义后端事件、领域事件结构。
- Replayability 声明。

### 如何协同

- Source 产生 Envelope。
- Dispatcher 发布 Envelope。
- Agent.accepts 与 Plugin.consumes 用 ScopeRule 匹配 Envelope。
- Scheduler / Strategy 处理已归属给 Agent 的 Envelope。

### 反模式

- 在 core 中解析某个外部协议 payload。
- Envelope 缺少 payload_schema_id，导致下游猜类型。
- 用自由文本替代结构化 payload 和 schema。

## 8. ScopeRule

### 含义

ScopeRule 是 Envelope 路由谓词，用于声明 Agent 或 Plugin 接收哪些输入。

### 包含内容

- BySchema / BySchemaPrefix。
- BySourceId / BySourceKind / BySourceField。
- ByCapability。
- AND / OR 谓词组合。
- 命名 scope。

### 必须内化

- Agent.accepts 过滤。
- Plugin.consumes 二次分发。
- 空 accepts 等价于拒绝所有 Envelope。

### 可被替换或扩展

- 领域命名 scope。
- 新 SourceKindName / CapabilityName 匹配条件。
- 组合规则，但应保持纯数据匹配，无副作用。

### 如何协同

- Dispatcher / AgentRegistry 使用 ScopeRule 筛选 Agent。
- Scheduler / Strategy 使用 ScopeRule 分发给插件。
- PermissionRule 负责“谁可调用”，ScopeRule 负责“是否路由”。

### 反模式

- 用 ScopeRule 表达权限。
- ScopeRule 执行 I/O 或读取可变外部状态。
- Agent 未声明 accepts 却隐式接收所有输入。

## 9. Dispatcher

### 含义

Dispatcher 是 Operation 调用与 Envelope 路由的核心入口。它位于 Agent 与插件
之间，插件通过 `ctx.dispatch` 使用它。

### 包含内容

- Operation 注册、查询、状态管理和调用。
- Source 注册、查询和校验。
- Envelope publish。
- 跨 Agent invoke。
- capability / permission / trace 拦截链。

### 必须内化

- Operation 调用顺序。
- Source 注册校验。
- inline await 快路径。
- Operation / Source 跟随 PluginScope 反注册。
- 结构化错误。

### 可被替换或扩展

- 具体 Operation handler。
- Agent election policy 的排序部分。
- 外部协议经由插件注册的 Operation / Source。

### 如何协同

- Agent 持有 Dispatcher。
- Plugin 注册 Operation / Source。
- Strategy 调用 Dispatcher。
- Observability 旁路消费 Dispatcher 产生的 trace。

### 反模式

- Dispatcher 内置 OneBot / MCP / ChatCompletion 等外部协议。
- Dispatcher 替插件执行领域业务。
- 为了性能绕开 capability / permission / trace。

## 10. AgentRegistry 与输入归属

### 含义

AgentRegistry 是进程内 Agent 注册表，负责发现当前可用 Agent、过滤生命周期与
accepts，并按策略排序。

### 包含内容

- Agent 弱引用注册表。
- awake + accepts 前置过滤。
- 默认 priority + agent_id 排序。
- 可插件化 election policy。
- 单主输入归属设计中的 owner selection。

### 必须内化

- 生命周期过滤。
- `Agent.accepts` 过滤。
- 策略只能排序已匹配候选，不能绕过过滤。
- 明确区分 single-owner 与 broadcast 语义。

### 可被替换或扩展

- 候选排序策略。
- source/session binding 存储实现。
- owner 选择策略，但必须保留确定性与可观测性。

### 如何协同

- Dispatcher.publish 使用 AgentRegistry 找到接收 Agent。
- 单主路由使用 AgentRegistry 选择 owner。
- Agent 间调用使用 registry 查找目标 Agent。
- Plugin 可安装 election policy，并通过 scope disposer 恢复默认。

### 反模式

- 策略插件绕过 Agent 生命周期。
- observer Agent 被选为 owner。
- 没有 binding 时重复选主导致同一 session 漂移。

## 11. AgentProfile 与 Participation

### 含义

AgentProfile 是对 Agent 角色与参与方式的声明。它不是 Agent 本身，而是 Agent
如何参与输入流、使用策略和副作用权限的配置层。

### 包含内容

- profile_id。
- participation：primary_candidate、observer、explicit_helper。
- accepts。
- strategy_id。
- side_effect_policy。

### 必须内化

- participation 的语义。
- observer 默认只读。
- explicit_helper 不自动接收外部输入。
- primary_candidate 才可成为输入 owner。

### 可被替换或扩展

- profile schema。
- side_effect_policy 细节。
- 领域 profile，如 yume-awake、bot-command、coding-main。

### 如何协同

- Agent 根据 profile 暴露 accepts 与策略。
- AgentRegistry 根据 participation 选择 owner 或旁路对象。
- Permission / Capability 根据 profile 限制副作用。
- Strategy 根据 profile 判断自己是否支持该 Agent。

### 反模式

- 把 Profile 当成 Agent 实例。
- 让插件随意修改当前 Agent participation。
- observer 默认拥有写外部状态权限。

## 12. Capability

### 含义

Capability 描述插件有能力做什么，是静态声明和资源量纲的入口。

### 包含内容

- 注册式 CapabilityName。
- quantity / policy。
- 插件 manifest capabilities。
- Operation requires_capabilities。

### 必须内化

- 未注册能力名拒绝。
- 未声明即调用拒绝。
- 核心只内置通用能力，不内置领域能力。

### 可被替换或扩展

- 领域能力命名空间，如 yume.vram。
- 资源量纲。
- ResourceHost / governor 的具体治理策略。

### 如何协同

- Plugin 声明 capability。
- Operation 声明 requires_capabilities。
- Dispatcher 调用时检查 capability。
- ScopeRule 可按 capability 匹配 Envelope。

### 反模式

- 裸字符串能力名。
- 在 core 中加入领域 capability。
- 用 capability 替代 permission。

## 13. Permission

### 含义

Permission 描述当下这个调用是否被允许，是动态谓词。它与 Capability 正交。

### 包含内容

- PermissionName。
- PermissionRule。
- AND / OR 组合。
- Operation / command perms。

### 必须内化

- 调用时检查。
- 失败返回结构化错误。
- 与 Agent owner / context 关联。

### 可被替换或扩展

- 新权限名。
- 领域权限谓词。
- 与 workspace、channel、session、owner 相关的规则。

### 如何协同

- Dispatcher 调用 Operation 前检查 Permission。
- Agent/Profile 可参与 permission 上下文。
- Capability 先声明“能做”，Permission 再判断“此刻能不能做”。

### 反模式

- 用 Permission 描述资源量纲。
- 权限失败后 fallback 成默认成功。
- 在插件里绕开 Dispatcher 自行判断。

## 14. Service

### 含义

Service 是跨插件共享能力或状态的具名接口。它通过契约暴露，而不是通过插件
实现模块暴露。

### 包含内容

- service name。
- contract_id / version。
- mode：by_value 或 by_ref。
- provider 与 consumer 声明。

### 必须内化

- 服务契约必须显式声明。
- by_ref 不可跨进程。
- 缺失服务应 fail-loud。

### 可被替换或扩展

- 服务实现。
- 服务契约包。
- by_value / by_ref 的具体后端。

### 如何协同

- Plugin 通过 provides_services / requires_services 声明服务关系。
- ServiceContainer 注入服务。
- Operation / Strategy 可通过 ctx.services 获取服务。
- ResourceHost 可作为服务暴露。

### 反模式

- 插件直接 import provider 插件实现。
- 服务无契约或版本。
- by_ref 服务被序列化或跨域传递。

## 15. ResourceHost / Handle / RefPayload

### 含义

ResourceHost 管理可跨 plugin reload 存活的进程内资源。Handle 表示引用所有权，
RefPayload 表示契约中按引用传递的字段。

### 包含内容

- ResourceHost 资源托管、租约、策略配置。
- Handle acquire / release / borrow / attach_to。
- RefDescriptor 元数据。
- RefPayload 字段标记。
- ResourceRecord 与 eviction / keepalive policy。

### 必须内化

- Handle 生命周期。
- 未 attach 的 Handle 视为泄漏。
- RefPayload 不可普通序列化。
- core 不解释 RefDescriptor.attributes 的领域含义。
- host / handle 操作发出 trace。

### 可被替换或扩展

- 具体资源类型。
- finalizer。
- ResourceHost policy。
- 领域 ref kind 与 schema。

### 如何协同

- Plugin 创建或借用资源。
- Agent / Strategy 通过服务或 RefArg 获取 Handle。
- Dispatcher / Dependency 解析 RefArg。
- Observability 记录资源 trace。

### 反模式

- 在 core 中出现 latent / tensor / gpu 专用字段。
- 插件字段直接持 raw socket / SDK client。
- RefPayload 跨进程传输。
- 策略长期持有未绑定 scope 的 Handle。

## 16. Runtime

### 含义

Runtime 提供执行环境能力，不决定 Agent 行为。

### 包含内容

- Clock。
- IdGen。
- RNG。
- 事件循环与 scheduler 包装。
- 同步点检查。
- 并发、工作池或隔离策略。

### 必须内化

- 决定性时间、ID、随机源由 runtime 注入。
- 禁止插件直接使用全局 time / uuid / random。
- 同步点必须显式。

### 可被替换或扩展

- 时钟实现。
- ID 生成器。
- 随机源。
- 事件循环策略。
- 测试替身。

### 如何协同

- AgentContext 暴露 runtime 能力。
- Tests 使用可控 clock/id/rng。
- Trace 使用 runtime 时间与 ID。
- Strategy / Plugin 只能通过 ctx 访问 runtime 能力。

### 反模式

- 插件直接调用 `time.time()` / `uuid.uuid4()` / `random`。
- Runtime 决定 Agent 行为。
- 隐式阻塞或裸跑 CPU 密集任务。

## 17. Observability

### 含义

Observability 是旁路观测层，记录 trace、audit、metrics。它不进入主链路依赖，
卸载 observability 不应影响 Agent 行为。

### 包含内容

- TraceSpan。
- JsonlTraceWriter / Reader。
- replay_trace_spans。
- contract test kit。
- audit / metrics 插件。

### 必须内化

- trace 因果字段。
- 每个命令、工具调用、Agent tick、生命周期切换都可观测。
- replay kit 尊重 Replayability。

### 可被替换或扩展

- trace sink。
- audit sink。
- metrics backend。
- replay 校验策略。

### 如何协同

- Dispatcher / ResourceHost / Scheduler 发 trace。
- Observability 订阅 trace 事件。
- Contract test kit 验证跨插件因果链与清理闭包。

### 反模式

- 主链路依赖 observability 插件。
- trace sink 失败导致 Agent 主流程失败。
- replay 假装能重放不可重放 payload。

## 18. Contract

### 含义

Contract 是 core、plugins、services、external bridge 之间共享的稳定协议。
协议优先于实现；没有契约位置的新机制不应直接塞进实现文件。

### 包含内容

- msgspec.Struct 协议对象。
- schema_id / schema_version。
- compatibility callback。
- Error。
- Command / Operation / Source / Envelope / RefPayload 等类型。

### 必须内化

- 契约版本与兼容性。
- 结构化错误。
- schema mismatch fail-loud。
- contracts-only 共享包机制。

### 可被替换或扩展

- 领域契约包。
- schema 兼容回调。
- 外部协议 bridge 中的外部 schema。

### 如何协同

- Plugin manifest 声明依赖契约。
- PluginLoader 做静态发现与 DAG 解析。
- Dispatcher / Dependency 根据契约执行调用与注入。
- Docs 与 tests 共同锁定契约行为。

### 反模式

- 先写实现再补契约。
- 用自由文本约定跨插件 payload。
- core 内置外部协议 schema。

## 19. 外部协议桥

### 含义

外部协议桥是插件，不是 core 层级。它负责把 OneBot、MCP、ChatCompletion、
浏览器、文件系统等外部协议翻译为 MutsukiBot 内部契约。

### 包含内容

- 外部连接或 SDK client，经 Handle / PluginScope 管理。
- Source：外部事件入口。
- Operation：外部动作出口。
- 外部 payload 到 Envelope / Operation payload 的转换。

### 必须内化

外部协议本身不进入 core。bridge 必须内化协议翻译细节，并对外暴露内部契约。

### 可被替换或扩展

- OneBot v11 / v12。
- MCP。
- ChatCompletion。
- 本地 shell、浏览器、文件系统。
- 任何未来 transport 或 tool endpoint。

### 如何协同

- Bridge plugin 注册 Source / Operation。
- Agent / Strategy 只看内部 Envelope / Operation。
- Dispatcher 负责路由和调用。
- PluginScope 负责资源清理。

### 反模式

- 在 Dispatcher / Agent / contracts 中出现外部协议专用字段。
- bridge 直接调用业务插件实现。
- 为某协议维护独立 adapter 层级，绕开 Plugin。

## 20. 典型协作流程

### 20.1 单主编程 Agent 输入

```text
External input
  -> bridge plugin publishes Envelope
  -> routing chooses primary Agent owner
  -> Agent receives Envelope
  -> ExecutionStrategy decides next step
  -> ctx.dispatch.invoke(tool op)
  -> Operation returns observation
  -> Strategy updates Agent state and emits Decision / reply
  -> Observability records trace
```

### 20.2 旁路观察

```text
External input or trace
  -> broadcast copy
  -> observer Agent / plugin consumes read-only
  -> writes own memory / audit state
  -> no user reply or external side effect by default
```

### 20.3 跨 Agent helper

```text
Primary Agent strategy
  -> ctx.dispatch.invoke_in_agent(helper_id, op_id, payload)
  -> helper Agent Dispatcher invokes Operation
  -> result returns inline
  -> trace parent-child chain preserved
```

### 20.4 资源借用

```text
Plugin creates resource
  -> ResourceHost stores Handle
  -> Operation receives ref_id / RefPayload
  -> Dependency resolves Handle via ResourceHost
  -> handler borrows resource
  -> release / finalizer tracked by scope and host
```

## 21. 放置规则

新增机制时按以下规则归位：

- 定义“谁拥有输入 / 状态 / 副作用” -> Agent / AgentProfile。
- 定义“下一步怎么推进” -> ExecutionStrategy / AgentPolicy。
- 定义“能做什么” -> Plugin / Operation / Service / Resource。
- 定义“输入从哪来、是什么” -> Source / Envelope / Contract。
- 定义“谁能收到输入” -> ScopeRule / AgentRegistry。
- 定义“此刻能不能调用” -> Permission。
- 定义“是否具备能力与资源量纲” -> Capability / ResourceHost。
- 定义“如何接外部协议” -> bridge plugin。
- 定义“如何观测” -> Observability。
- 定义“时间、ID、随机与同步点” -> Runtime。

如果一个机制同时想占多个位置，优先拆开，而不是创造一个万能概念。

## 22. 统一反模式清单

- Agent 退化为插件容器。
- ExecutionStrategy 拥有生命周期或权限。
- Plugin 直接 import 兄弟插件实现。
- Operation 与命令 / tool 维护两份实现。
- 外部协议进入 core / contracts。
- Source 带 handler 或业务状态。
- ScopeRule 执行副作用。
- Capability 被当成 Permission。
- ResourceHost 解释领域 metadata。
- Runtime 决定 Agent 行为。
- Observability 失败影响主链路。
- 没有契约就把机制写进实现文件。
