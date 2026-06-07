# MutsukiBot 文档

> 当前文档对应已交付的 **v0.4 runtime**。本套文档对应仓库内已经实现的代码，不是路线图。
> 尚未落地的扩展（跨进程隔离、完整资源调度、LLM provider 等），见 [附录 · 未实现 / 路线图](appendix/roadmap-and-not-yet.md)。

MutsukiBot 是一个 **Agent 中心** 的 Bot 框架，给 Yume / mind-sim 提供运行核心，同时以插件组合的方式覆盖传统 Bot 框架的能力。文档章节按从浅到深排列；如果是第一次接触，按顺序读 01 → 02 → 03 即可在十分钟内跑通；如果要写自己的插件或排查机制问题，直接跳到 04。

## 章节导航

### [01. 介绍](01-introduction/)

- [什么是 MutsukiBot](01-introduction/what-is-mutsukibot.md) —— 一句话定位、分层、v0.2 已交付能力
- [设计哲学与硬规则](01-introduction/design-philosophy.md) —— 为什么 Agent 是一等公民、为什么 core domain-neutral
- [与 NoneBot 的对比](01-introduction/comparison-with-nonebot.md) —— 心智借鉴在哪、刻意分歧在哪

### [02. 安装](02-installation/)

- [安装与冒烟](02-installation/installation.md) —— Python 3.13 / uv / pytest / 跑一遍 echo

### [03. 快速上手](03-quickstart/)

- [跑通 Echo](03-quickstart/run-echo.md) —— 用 InMemoryEndpointPlugin 走完一遍消息闭环
- [第一个插件](03-quickstart/first-plugin.md) —— 抄着 EchoPlugin 写一个 greet

### [04. 指南：核心机制](04-guide/)

按子系统讲清「这个东西是什么、解决什么、怎么工作的」。**这是文档的重头戏。**

- [Agent 与生命周期](04-guide/agent-and-lifecycle.md)
- [AgentContext](04-guide/agent-context.md)
- [插件定义与 PluginMeta](04-guide/plugin-definition.md)
- [命令与 Schema 生成](04-guide/command-and-schema.md)
- [依赖注入](04-guide/dependency-injection.md)
- [服务容器](04-guide/service-container.md)
- [PluginScope 与资源回收](04-guide/plugin-scope.md)
- [事件总线](04-guide/event-bus.md)
- [Capability](04-guide/capability.md)
- [Permission](04-guide/permission.md)
- [Error 模型](04-guide/error-model.md)
- [Handle 与 RefPayload](04-guide/handle-and-refpayload.md)
- [Trace 与 Span](04-guide/trace-and-span.md)
- [ResourceHost](04-guide/resource-host.md)

### [05. 进阶](05-advanced/)

- [插件 DAG 加载](05-advanced/plugin-loader-dag.md)
- [热重载与泄漏检测](05-advanced/hot-reload-and-leak.md)
- [TransactionScope 与 Saga](05-advanced/transaction-scope-saga.md)
- [确定性运行时与可重放](05-advanced/deterministic-runtime.md)
- [注册式字符串扩展](05-advanced/registered-strings.md)

### [06. 开发者指南](06-developer/)

- [写一个 transport plugin](06-developer/writing-transport-plugin.md)
- [自定义运行时（Clock / IdGen / RNG）](06-developer/writing-runtime.md)
- [测试夹具](06-developer/testing-fixtures.md)

### [07. API 参考](07-api/)

按模块整理公开符号。文档只做地图，权威源是源码。

- [`mutsukibot`](07-api/mutsukibot.md)
- [`mutsukibot.contracts`](07-api/contracts.md)
- [`mutsukibot.core`](07-api/core.md)
- [`mutsukibot.runtime`](07-api/runtime.md)
- [`mutsukibot.core.dispatcher`](07-api/dispatcher.md)
- [Transport endpoints](07-api/endpoint.md)
- [`mutsukibot.plugins`](07-api/plugins.md)

### [附录](appendix/)

- [术语表](appendix/glossary.md)
- [未实现 / 路线图](appendix/roadmap-and-not-yet.md)

## 文档约定

- 凡引用源码都用相对链接，例如 [agent.py:74](../mutsukibot/core/agent.py#L74)。
- 类、函数、命令名首次出现用反引号包起来；后续可省。
- 所有代码片段都从 `mutsukibot/` 现有实现里截取或仿写，可直接运行。
- 文档正文记录当前已交付 runtime；任何带「计划中」「未来」字样的描述都集中在 [附录 · 未实现](appendix/roadmap-and-not-yet.md)。

## 如何贡献文档

文档与代码同 PR 改动。以下场景**必须**同步修改文档：

1. 新增 / 删除 / 重命名 `mutsukibot/__init__.py` 或 `mutsukibot.contracts/__init__.py` 的导出符号
2. 修改任何 `Plugin` / `Agent` / `AgentContext` / `PluginScope` / `Bus` / `AgentScheduler` 的公共方法签名
3. 引入新的 hard rule 或 lint 规则
4. 调整插件加载、调度、错误分类的行为

具体写作风格遵循 [04 指南](04-guide/) 的五段式模板：**这是什么 / 解决什么 / 怎么工作 / 用法示例 / 常见陷阱**。
