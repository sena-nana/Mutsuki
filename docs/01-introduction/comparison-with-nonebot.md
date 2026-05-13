# 与 NoneBot 的对比

NanoBot 在心智上明显借鉴 NoneBot —— `Dependent` 思路、`Rule` / `Permission` 思路、装饰器风格、event 总线 —— 但在所有具体形态上都做了刻意分歧。本文逐项对照。

## 心智借鉴

| 概念 | NoneBot 怎么做 | NanoBot 怎么做 |
|---|---|---|
| 依赖注入 | `Depends(...)` 默认值，按签名解析 | `Dependent[R]` + 4 类 `Param`（[详见](../04-guide/dependency-injection.md)）|
| 权限组合 | `Rule` 与 `Permission` 两个独立概念，支持 `&` `\|` | 合并成单类型 `PermissionRule`（[详见](../04-guide/permission.md)）|
| 装饰器风格 | `@on_command(...)` / `@on_message(...)` | `@command(...)`（[详见](../04-guide/command-and-schema.md)）|
| 事件总线 | `Bot` 持有 driver；事件由 driver 派发 | `Bus` 是 Agent 自带（direct + deferred 两路，[详见](../04-guide/event-bus.md)）|
| 插件加载 | `nonebot.load_plugin(...)` | `PluginLoader` + entry_points + DAG 拓扑（[详见](../05-advanced/plugin-loader-dag.md)）|

## 刻意分歧

### Bot 是会话 vs Agent 是常驻

NoneBot 的 `Bot` 实例与一个平台 connection 绑定：QQ bot 是一个 Bot，Discord bot 是另一个。Bot 本身不太持有"内在状态"——状态在 session、user、group 这些会话级对象。

NanoBot 的 `Agent` 不绑定平台 —— 它是常驻运行时实体，**adapter 是它面向外部的窗口**。一个 Agent 可以同时通过多个 adapter 接收消息（CLI + WS + HTTP）；状态由 Agent 自身持有。

### 一份签名两个用途

NoneBot 里"命令"与"LLM tool"是两个独立概念，没有内置桥。NanoBot 把它们合并：`@command(is_tool=True)`（默认）的方法**同时**是命令路由目标与 LLM tool 描述源。Schema 由元类一次合成。

### Capability 静态声明

NoneBot 没有 capability 概念 —— 一个插件能不能调用网络、能不能写文件，靠人工约定。NanoBot 强制 manifest 里声明 `capabilities=[Capability(name=Caps.NETWORK_EGRESS), ...]`；命令额外声 `requires_capabilities`；调度器在分发前校验。

### 插件之间禁止直接 import

NoneBot 的最佳实践推荐用 `nonebot.plugin.export` 共享接口，但底层不强制 —— 你可以 `from other_plugin.internals import xxx`。NanoBot hard rule #3 直接禁止：插件只能通过契约 + 服务通信。

### Scope 化资源回收

NoneBot 卸载插件时，未清理的订阅、定时器、handle 没有强制约束 —— 由插件作者自己保证。NanoBot 的 `PluginScope` 是强制接口：所有副作用必须登记，scope.close 自动反向回收，泄漏抛 `HandleLeakError`。

### 决定性时间与 ID

NoneBot 没有这条约束 —— 插件随便 `time.time()`。NanoBot hard rule #9 把 Clock / IdGen / RNG 全注入式，插件只能通过 `ctx.clock` / `ctx.id_gen` / `ctx.rng` 访问 —— 让"同输入 → 同 trace"成为可能。

### 结构化错误

NoneBot 用 Python 标准异常体系。NanoBot 用 `Error` 一等数据对象，scheduler 把命令里抛的异常分类映射成 `Error` 投到出站 / trace —— 错误可路由、可断言、可指标化。

## 概念对照表

| NoneBot 名 | NanoBot 名 | 备注 |
|---|---|---|
| Bot | Agent | NoneBot 的 Bot 偏向"平台连接"；NanoBot 的 Agent 是常驻运行时实体 |
| Driver | （Adapter + AgentScheduler 共同承担） | NoneBot 的 Driver 兼管协议适配与事件循环；NanoBot 把这两件分给 Adapter 与 AgentScheduler |
| Adapter | Adapter | 概念基本一致 |
| Matcher | （没有等价物） | NanoBot 没有"匹配规则"层 —— 调度直接按命令名 dict 路由 |
| Rule / Permission | PermissionRule + PermissionName | 两件合并 |
| Depends | Inject() / RefArg() / 类型注解推断 | 取消按名 fallback |
| Event | Message + Event（contracts 层） | NanoBot 的 Message 与 NoneBot 的 Event 概念相近；NanoBot 还有更通用的 `Event` 契约（含 trace 三段）|
| Plugin | Plugin（类） | NoneBot 插件是模块；NanoBot 插件是类，元类校验 |
| 命令解析 | Schema 强转 | NoneBot 用专门的命令解析器；NanoBot scheduler 按 parameters_schema 类型粗粒度强转（int/float/bool）|
| logger | TraceSpan + Bus | NanoBot 把 trace 做成结构化事件，业务自己写日志另说 |

## 你应该选哪个

**用 NoneBot**：

- 你需要立刻接入 QQ / 微信 / Discord / OneBot 等平台 —— NoneBot 的生态远大于 NanoBot
- 你的 bot 是无状态命令响应器，不需要"agent 内在状态"
- 你不想把所有时间 / ID / 随机数都改成 `ctx.*` 调用

**用 NanoBot**：

- 你想给 Yume / mind-sim 类的 agent 系统做运行核
- 你需要插件热重载并且 100% 不漏资源
- 你需要"命令与 LLM tool 共用一份定义"
- 你愿意为可重放性投资类型注解 + ctx 注入风格

**借鉴 NanoBot 的形态去改造你的 NoneBot 项目**：

- `PluginScope` 模式（自己包装 `nonebot.on_subscribe(...)` 的解订阅）
- `Capability` 静态声明（自定义 manifest 字段 + CI 校验）
- 决定性时钟 / ID（自己注入 Clock 进 session）

## 一些 NoneBot 用户读 NanoBot 文档时的预期偏差

- **没有 Driver**——它的职责被拆给了 Adapter（协议）+ AgentScheduler（tick 循环）
- **没有 Matcher**——命令路由直接按 dict；没有"匹配规则栈"层
- **没有 `nonebot.get_bot()`**——Agent 没有全局取实例的接口；要拿当前 ctx
- **没有 require / declare 层**——插件依赖通过 `requires_plugins` / `requires_services` 走 DAG，不是 `require()` 调用
- **`@command` 不接受字符串别名列表**——别名靠注册多个 attr_name 实现，或者业务侧做命令名映射
- **没有 `bot.send(...)`**——返回值被 scheduler 包成出站消息，自己往 outbox 推；要异步发其他消息可以 `await ctx.bus.publish(...)` 让 adapter 订阅
