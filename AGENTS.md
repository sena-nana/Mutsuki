# MutsukiBot v0.1 项目宪法

MutsukiBot 是一个全新的 **Agent runtime kernel**，融合 Koishi、NoneBot、AstrBot 的优势但不把 Bot / IM / command 语义放进核心。当前代码已将 IM 与文本命令路径外抽为 reference extension。

## 一句话定位

为 Yume / mind-sim 与 Lilia 式工程 Agent 提供运行核心，并通过 extension / 插件组合实现传统 Bot 框架能力。Yume / mind-sim 自身的实现路径也将被解构为本框架之上的零散插件。

## 阅读顺序

任何变更前，按以下顺序阅读相关文档：

1. [plans/roadmap.md](plans/roadmap.md) —— 当前版本目标、门控、范围。
2. [plans/architecture.md](plans/architecture.md) —— 方向、Agent 一等公民、分层、与 Yume / mind-sim 的关系。
3. [plans/engineering.md](plans/engineering.md) —— 技术栈、目录结构、插件模型、横切公约。
4. [plans/contracts.md](plans/contracts.md) —— 内部协议草案（v0.0 必须先有契约才能写 core 实现）。
5. 既有契约、实现、测试。

任何变更若没有契约位置或设计文档归属，**先设计或更新契约**，禁止直接把新机制塞进实现文件。

## 不可违反的最高规则（Hard Rules）

以下规则贯穿全框架，违反即拒绝合入。详细解释见对应文档章节。

1. **Agent 是一等运行时实体** —— 拥有身份、Context、生命周期、独立调度循环；Agent ≠ 会话 ≠ LLM 调用。
2. **核心不内置业务概念** —— LLM 调用、记忆、情感、睡眠、消息平台都必须是插件，不在 `core` 中实现。
3. **插件之间禁止直接 import 实现模块** —— 只能通过契约 + 服务通信。
4. **无副作用热重载** —— 卸载必须回收所有副作用；未通过 `PluginScope` 注册的副作用即视为违规。
5. **Operation 即工具** —— 同一个 Operation 声明可同时服务「人类可触发入口」与「Agent / LLM 可调用工具」manifest，禁止维护两份；文本 command 只是 reference extension 入口。
6. **无 schema 的插件不允许装载** —— 必须用 `msgspec.Struct` 声明 config schema。
7. **未申报 capability 即调用视为违规** —— Capability 必须在 manifest 中显式列出。
8. **结构化错误，不允许吞异常返默认值** —— fallback 必须显式记录原因。
9. **决定性时间与 ID 由 runtime 注入** —— 插件禁止直接用 `time.time()` / `uuid.uuid4()` / `random` 全局源。
10. **同步点显式化** —— 禁止隐式阻塞，必须走 runtime scheduler。
11. **双协议分离** —— 外部协议（OneBot / MCP / ChatCompletion 等）只能出现在 reference plugin（v0.2 起取代旧 adapters）中，不得渗透 `core` / `contracts`。
12. **Borrow with Discipline** —— 借鉴 Koishi / NoneBot / AstrBot 的心智，**不照搬代码或 API 形态**；每个机制必须能解释自己对「Agent 一等公民、解耦、可扩展」中至少一项的贡献。
13. **未声明 accepts 即拒绝路由**（v0.2 引入）—— Agent 必须显式声明 `accepts: tuple[ScopeRule, ...]`；空 tuple 等价于不接收任何 envelope（命令路径仍可用）。理由：路由层等价于 hard rule #8 的"显式 fallback 而非默认隐式"，避免 Agent 不自知地处理无关数据。详 [contracts.md §17](plans/contracts.md)。
14. **I/O 资源外置**（v0.2 引入）—— 所有 Plugin（含注册 Operation/Source 的）禁止字段直接持 raw socket / SDK client / 连接对象；必须通过 `Handle[T]` attach 到 `PluginScope`，由 finalizer 释放。理由：让"重载 plugin 逻辑"与"重连资源"在生命周期上解耦；为 v0.3+ 的 ResourceHost 跨 plugin reload 共享留出无迁移成本的接口。详 [plans/engineering.md §4.13](plans/engineering.md)。

## 工作准则

- **代码即事实，plans 是契约 + 决策**。当公共契约、插件协议、生命周期阶段、服务接口发生变化时，**同 PR 内**更新 `plans/`。
- **不得以部分检查宣称成功**。报告精确执行的验证命令与结果。
- **plans 保持精简**。过期讨论删除，但接口契约与决策必须保留。
- **每个 v0.x 完成时产出** `plans/version-reports/v0.x.md`：方向、完成项、基线、运行检查、效果检查、下版门槛。

## 技术栈

Python 3.13 + uv + asyncio。详见 [plans/engineering.md](plans/engineering.md)。
