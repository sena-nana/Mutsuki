# 术语表

MutsukiCore 文档里高频出现的概念按字母序速查。链接指向最详细的解释位置。

## A

**Adapter** —— v0.1 的旧抽象，v0.2 已删除。外部传输现在写成 transport plugin，通过 Source / Operation 接入。详见 [写一个 transport plugin](../06-developer/writing-transport-plugin.md)。

**Agent** —— MutsukiCore 的一等运行时实体。有身份、生命周期和调度边界。当前 Rust 主链由 `AgentRuntime` 维护 lifecycle / routing / tick；Python reference 层由 `AgentScheduler` 驱动兼容 tick 循环。详见 [Agent 与生命周期](../04-guide/agent-and-lifecycle.md)。

**AgentContext** —— 单次调用上下文。插件命令以 `ctx` 形式接收，是访问 clock / id_gen / rng / services / scope / bus / trace 的唯一入口。详见 [AgentContext](../04-guide/agent-context.md)。

**AgentScheduler** —— Python reference 层驱动旧 `Agent` `asyncio` tick 循环的对象。从 `inbox` 取消息，路由到命令，把结果发到 `outbox`；Operation 执行 trace 由 dispatcher 统一产出。当前 Rust core 中没有公开名为 `AgentScheduler` 的类型，对应能力由 `AgentRuntime` 方法与 host tick loop 表达。详见 [API · runtime.scheduler](../07-api/runtime.md#scheduler)。

**Arg** —— 命令参数的 `Annotated` 元数据，承载约束（min/max/regex/choices）+ 兜底描述。详见 [命令与 Schema](../04-guide/command-and-schema.md)。

## B

**Bus** —— Agent 持有的进程内事件总线。两种派发模式：默认 deferred（`asyncio.gather` 并发），快路径 direct（内联 await）。详见 [事件总线](../04-guide/event-bus.md)。

**by-ref / by-value** —— 服务模式枚举（`ServiceMode`）。by-ref 持有不可序列化对象（GPU 句柄等），by-value 是普通可序列化数据。v0.1 容器不强制区分，仅 manifest 元数据。

## C

**Capability** —— 插件静态声明的"我能做什么"。注册式字符串 + 可选 quantity / policy。详见 [Capability](../04-guide/capability.md)。

**CapabilityName** —— 已注册的能力名（`str` 子类）。用 `CapabilityName.register(name, declared_by=...)` 扩展。

**Caps** —— 内置 CapabilityName 常量门面。`Caps.READ_MESSAGE` 等。

**ChannelRef** —— Message 的来源指针（source_id + channel_id + 可选 user_id）。

**command（装饰器）** —— 把 async 方法标记为命令 + LLM tool。详见 [插件定义](../04-guide/plugin-definition.md)。

**CommandSpec** —— 已编译的命令规约（name / description / parameters_schema / requires_capabilities ...），由 PluginMeta 在类定义时合成。

**Contract** —— 所有 MutsukiCore 契约对象的基类（`msgspec.Struct` 子类），要求 ClassVar `schema_id` / `schema_version`，自动注册到 `SchemaRegistry`。

## D

**Decision** —— 记录"选了什么、考虑过什么备选"的契约。

**Dependent** —— 类型化依赖注入对象。在类定义时把签名拆成 `Param` 列表，调用时按列表求值。详见 [依赖注入](../04-guide/dependency-injection.md)。

**direct dispatch / deferred dispatch** —— Bus 的两种派发模式。direct 内联串行，deferred 并发。

## E

**Error** —— 结构化错误对象。含 code / source / route / cause / evidence。详见 [Error 模型](../04-guide/error-model.md)。

**ErrorCode** —— 已注册的错误码（`str` 子类）。

**Errs** —— 内置 ErrorCode 常量门面。14 个内置码。

**Event** —— 跨插件的内部事件契约（含 trace 三段）。

## H

**Handle** —— 引用计数的所有权抽象，持有不可序列化对象。`acquire` / `release` / `borrow` / `attach_to(scope)`。详见 [Handle 与 RefPayload](../04-guide/handle-and-refpayload.md)。

**HandleLeakError** —— scope 关闭时仍有未释放 handle，或 cleanup 失败时抛出。携带结构化 `Error(code=Errs.HANDLE_LEAK)`。

**Hard Rule** —— [AGENTS.md](../../AGENTS.md) 的 12 条不可违反规则。详见 [设计哲学](../01-introduction/design-philosophy.md)。

## I

**IdGen** —— ID 生成协议。`NanoIdGen`（生产）/ `DeterministicIdGen`（测试）。

**Inject** —— 命令签名里的服务注入 sentinel：`svc: SomeService = Inject()`。

**InMemoryEndpointPlugin** —— 进程内 IM endpoint reference plugin，测试 / 冒烟用。

## L

**Lifespan** —— Agent 生命周期钩子集合（on_spawn / on_awake / on_sleep / on_stop）。

**LifecyclePhase** —— Agent 阶段枚举：SPAWN → AWAKE → SLEEP → STOP。

## M

**Message** —— 入站 / 出站消息契约。含 id / timestamp / source（ChannelRef）/ parts（ContentPart 元组）。

## N

**NanoIdGen** —— 生产 IdGen 实现。`<prefix>_<26 字符 base32>`。

## P

**Param** —— `Dependent` 里的参数解析器抽象基类。四个具体子类：CtxParam / ArgParam / ServiceParam / RefParam。

**Permission** —— 命令运行时准入谓词。与 capability 正交。详见 [Permission](../04-guide/permission.md)。

**PermissionName** —— 已注册的命名权限（携带 checker）。

**PermissionRule** —— 可组合的权限谓词 AST，支持 `&` / `|`。

**Perms** —— 内置 PermissionName 门面。`Perms.PUBLIC` / `Perms.AGENT_OWNER`。

**Plugin** —— 插件基类（`Generic[Config]`）。继承它，声明 ClassVar id / version / capabilities + 嵌套 Config。详见 [插件定义](../04-guide/plugin-definition.md)。

**PluginMeta** —— Plugin 的元类。在 class 定义时校验、收集命令、构造 manifest、登记到 PluginRegistry。

**PluginManifest** —— 静态插件元数据契约（id / version / capabilities / commands ...）。

**PluginScope** —— 插件副作用作用域。订阅 / 定时器 / 服务注册 / handle 都登记到它，close 时反向回收。详见 [PluginScope](../04-guide/plugin-scope.md)。

## O

**OneBotV11Plugin** —— OneBot v11 反向 WebSocket reference plugin。只在 plugin 内处理 OneBot 外部协议字段。

**Operation** —— 命令、跨 plugin RPC、外部调用面的统一概念。由 `OperationDescriptor` 静态声明，通过 `Dispatcher.invoke` 执行。

## R

**RefArg** —— 命令签名里的 handle 参数标记：`Annotated[Handle[T], RefArg(kind="...")]`。

**RefDescriptor** —— Handle 的可观测元数据（ref_id / kind / schema_id_target / attributes / lineage）。永远可序列化。

**RefPayload** —— 契约层字段标记，表明字段通过引用持有。

**RegisteredString** —— `CapabilityName` / `PermissionName` / `ErrorCode` 共用的注册式 str 基类。详见 [registered-strings](../05-advanced/registered-strings.md)。

**RNG** —— 可种子化随机数生成器协议。`SeededRng` 是默认实现。

## S

**Saga** —— 多步事务编排，每步 `(forward, compensate)`。失败时自动反向补偿。详见 [TransactionScope 与 Saga](../05-advanced/transaction-scope-saga.md)。

**Scope** —— 见 PluginScope / TransactionScope。

**ServiceContainer** —— 按 `(契约类型, 可选名字)` 索引的服务注册表。详见 [服务容器](../04-guide/service-container.md)。

**Source** —— 事件来源声明。transport plugin publish envelope 前必须注册 Source，`Envelope.source.source_id` 指向它。

**SpanStatus** —— TraceSpan 的状态枚举：OK / ERROR。

## T

**TraceContext** —— `AgentContext` 的因果链字段：trace_id / span_id / parent_span_id。详见 [Trace 与 Span](../04-guide/trace-and-span.md)。

**TraceSpan** —— 单个调用的 trace 记录契约。dispatcher、ResourceHost、envelope consumer 等运行时入口会 emit。

**TransactionScope** —— PluginScope 的子类，加上 commit / rollback 与"补偿动作"语义。
