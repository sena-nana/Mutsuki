# 什么是 MutsukiBot

## 一句话定位

**MutsukiBot 是一个 Agent 中心的 Bot 框架，给 Yume / mind-sim 提供运行核心，同时通过插件组合复刻传统 Bot 框架的能力。**

—— 摘自 [README.md](../../README.md)。

## 与传统 Bot 框架的差别

传统框架（Koishi / NoneBot / AstrBot 等）的核心叫 "Bot" —— 它通常是无状态回调的容器：消息到了，路由到匹配规则，跑业务，回复。会话状态外接持久化层。

MutsukiBot 的核心叫 "Agent"：

- 有自己的 `agent_id`、生命周期阶段、独立调度循环
- 拥有自己的 ServiceContainer / Bus / inbox / outbox
- 是常驻对象，可以 spawn → awake → sleep → stop
- 持有的资源（订阅 / 句柄 / 显存）通过 `PluginScope` 自动回收

理由：要承载 Yume / mind-sim 这类**有内在状态、需要主动行动、长时间运行**的 agent，会话语义不够 —— 必须有一个明确的、可以被 spawn / awake 的对象。

但 MutsukiBot 不是只服务 Yume。任何"消息进来 → 命令路由 → 调插件 → 回响应"的传统 bot 形态，都能用 EchoPlugin 那种插件写法实现。Agent 与 Plugin 解耦：核心域中立，业务全在插件里。

## 分层

```
adapters → core → contracts ← plugins
                     ↑
              runtime（横向支撑）

observability ╌╌> （仅 pub/sub，不被任何层依赖）
```

来源：[plans/architecture.md](../../plans/architecture.md)。

| 层 | 职责 | v0.1 内容 |
|---|---|---|
| `contracts/` | 稳定的内部协议（仅类型，无运行时副作用） | Message / Event / Capability / Permission / Error / RefPayload / PluginManifest / ... |
| `core/` | Agent 运行时、注册表、调度器、容器、scope、loader | Agent / PluginMeta / Bus / PluginScope / ServiceContainer / Saga / ... |
| `runtime/` | 横向支撑：Clock / IdGen / RNG / Scheduler | SystemClock + ManualClock / NanoIdGen + DeterministicIdGen / SeededRng / AgentScheduler |
| `adapters/` | 协议翻译（CLI / WS / OneBot ...）—— 无业务逻辑 | InMemoryAdapter（测试用）|
| `plugins/` | 一切可装可卸的能力（命令、记忆、LLM、Yume 模块） | EchoPlugin（参考实现）|
| `observability/` | trace / audit / metrics —— 通过 bus 订阅，不被任何层依赖 | JsonlTraceWriter |

## v0.1 已交付能力

完整列表在 [plans/version-reports/v0.1.md](../../plans/version-reports/v0.1.md)。简版：

- **Agent 闭环**：`spawn → awake → 处理 echo → sleep → stop`
- **PluginMeta + @command**：类定义时校验、收集、构造 manifest，插件作者只写最小代码
- **依赖注入**：`Dependent[R]` + 4 类 `Param`（CtxParam / ArgParam / ServiceParam / RefParam）
- **PluginScope + 泄漏检测**：100 次热重载无泄漏（已通过门控）
- **Capability + Permission 注册式**：内置门面 `Caps` / `Perms`，扩展自有命名空间一行注册
- **结构化错误**：`Error` 一等数据 + 14 个内置 `ErrorCode`
- **Handle / RefPayload 协议**：`RefCountedHandle` + `make_stub_handle` 测试夹具
- **确定性运行时**：`ManualClock` + `DeterministicIdGen` + `SeededRng` 让测试可重放
- **Trace 因果链**：每条命令一个 `TraceSpan`；`JsonlTraceWriter` 旁路落盘
- **45 个测试**：覆盖契约、core、runtime、plugin 全路径
- **lint + 双类型检查**：ruff + pyright + pyrefly，CI 必须三者都通过

## v0.1 不在范围里的

按 [plans/roadmap.md](../../plans/roadmap.md) 与 [v0.1 报告](../../plans/version-reports/v0.1.md) 的标注，以下属于 v0.2 及之后：

- 真实平台 adapter（OneBot / WS）
- 配置文件加载与 schema 校验
- 运行时同步点检测（v0.1 留了 `install_sync_point_guard` 占位钩子）
- LLM provider 集成
- 持久化层
- Web 控制面板
- Yume 任何具体插件

详细列表见 [附录 · 未实现](../appendix/roadmap-and-not-yet.md)。

## 谁应该用 MutsukiBot

适合：

- 想给 Yume / mind-sim 类项目做"运行核"的人
- 写传统 bot 但希望 agent 持续在线、能保有内在状态的人
- 想要"插件热重载且不漏资源"作为强约束的人
- 想要"命令路由与 LLM tool 共用一套定义"的人

不适合：

- 想要 NoneBot 那种成熟生态、上百个 adapter 现成可用的人 —— v0.1 的 adapter 只有 InMemoryAdapter
- 想跑大规模分布式 bot 集群的人 —— v0.1 是单进程内
- 不想写类型注解的人 —— MutsukiBot 的 PluginMeta / Dependent 完全依赖类型注解工作

## 下一步

- 想跑起来 → [安装](../02-installation/installation.md) → [跑通 Echo](../03-quickstart/run-echo.md)
- 想理解为什么这么设计 → [设计哲学](design-philosophy.md)
- 已经熟悉 NoneBot，想知道差别 → [与 NoneBot 的对比](comparison-with-nonebot.md)
