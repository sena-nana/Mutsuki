# 设计哲学与硬规则

> 本文复述 [AGENTS.md](../../AGENTS.md) 与 [plans/architecture.md](../../plans/architecture.md) 的核心立场。要理解 MutsukiBot 为什么长这样而不是 NoneBot / Koishi 的样子，先读这一篇。

## 12 条 Hard Rules

[AGENTS.md](../../AGENTS.md) 列了 12 条不可违反的规则。每个 PR 评审都对照，违反即拒合入：

1. **Agent 是一等运行时实体**——拥有身份、Context、生命周期、独立调度循环；Agent ≠ 会话 ≠ LLM 调用
2. **核心不内置业务概念**——LLM 调用、记忆、情感、睡眠、消息平台都必须是插件，不在 `core` 中实现
3. **插件之间禁止直接 import 实现模块**——只能通过契约 + 服务通信
4. **无副作用热重载**——卸载必须回收所有副作用；未通过 `PluginScope` 注册的副作用即视为违规
5. **指令即工具**——同一个函数声明同时生成「人类可触发命令」与「Agent / LLM 可调用工具」manifest，禁止维护两份
6. **无 schema 的插件不允许装载**——必须用 `msgspec.Struct` 声明 config schema
7. **未申报 capability 即调用视为违规**——Capability 必须在 manifest 中显式列出
8. **结构化错误，不允许吞异常返默认值**——fallback 必须显式记录原因
9. **决定性时间与 ID 由 runtime 注入**——插件禁止直接用 `time.time()` / `uuid.uuid4()` / `random` 全局源
10. **同步点显式化**——禁止隐式阻塞，必须走 runtime scheduler
11. **双协议分离**——外部协议（OneBot / MCP / ChatCompletion 等）只能出现在 transport plugin / 桥接插件中，不得渗透 `core` / `contracts`
12. **Borrow with Discipline**——借鉴 Koishi / NoneBot / AstrBot 的心智，**不照搬代码或 API 形态**；每个机制必须能解释自己对「Agent 一等公民、解耦、可扩展」中至少一项的贡献

## 这些规则在代码里的对应

| 规则 | 落地点 |
|---|---|
| #1 Agent 一等 | [`Agent`](../../mutsukibot/core/agent.py) dataclass + [`AgentScheduler`](../../mutsukibot/runtime/scheduler.py) |
| #2 核心域中立 | [tests/contracts/test_no_domain_leakage.py](../../tests/contracts/) 强制 core 不出现 latent / vram 等领域字样 |
| #3 契约通信 | [`ServiceContainer.resolve`](../../mutsukibot/core/container.py) 按契约类型解析；契约类在 [`mutsukibot.contracts`](../../mutsukibot/contracts/) 独立模块 |
| #4 无副作用热重载 | [`PluginScope.close`](../../mutsukibot/core/scope.py) 反向清理 + 泄漏检测；100 次反复装卸的回归用例 |
| #5 指令即工具 | [`_build_command_spec`](../../mutsukibot/core/plugin.py) 从一份签名同时合成命令 schema 与 LLM tool schema |
| #6 必有 schema | [`PluginMeta`](../../mutsukibot/core/plugin.py) 在类定义时校验嵌套 `Config(msgspec.Struct)` 必存 |
| #7 显式 capability | [`check_capabilities`](../../mutsukibot/core/capability_guard.py) 在调度时 enforce required ⊆ declared |
| #8 结构化错误 | [`Error`](../../mutsukibot/contracts/error.py) 是 Contract（msgspec.Struct）；scheduler 把 Python 异常分类成 `Error` |
| #9 注入式 runtime | [`Agent.__init__`](../../mutsukibot/core/agent.py) 强制传入 `clock` / `id_gen` / `rng`；插件从 `ctx.*` 拿 |
| #10 同步点显式 | [`runtime/loop.py`](../../mutsukibot/runtime/loop.py) 留了 `install_sync_point_guard` 钩子；当前靠 ruff ASYNC 规则间接覆盖 |
| #11 双协议分离 | 外部协议只出现在 reference transport plugin；core / contracts 没有任何外部协议字样 |
| #12 借鉴有度 | NoneBot 的 `Dependent` 思路保留，但去掉按名 fallback；NoneBot 的 `Permission` 思路保留，但合并成单类型 `PermissionRule` |

## 几个反复回响的设计选择

### 为什么 Agent 是一等公民

如果"bot"是回调集合，Yume / mind-sim 这种"想要持续在线、有内在状态、能在没有外部触发时主动行动"的对象就**没地方住**——你只能把状态外接到数据库，每次回调时拉出来再写回去。这等于把 agent 退化成一个 stateful function。

把 Agent 做成对象有三个好处：

1. **自然承载内在状态**——agent 持有 services、scope、bus、句柄，卸载 agent = 卸载它的全部资源
2. **生命周期可观察**——`phase` 让外部代码区分"这个 agent 现在能不能用"
3. **可主动行动**——agent 自己有一个 tick 循环，可以从外部触发也可以从内部 timer 触发

### 为什么 core 必须 domain-neutral

核心代码里如果出现 `latent` / `vram` / `kv_cache` 字样，意味着核心已经"知道" Yume 在做什么 —— 一旦 Yume 内部架构调整，核心也要改。把领域语义全推给插件 + 通过 `RefDescriptor.kind` / `attributes` 自由扩展，核心永远只看通用形状。

[tests/contracts/test_no_domain_leakage.py](../../tests/contracts/) 把这条规则做成回归测试 —— 任何往 core 里塞领域字样的 PR 都会被它拦下来。

### 为什么 Capability 是注册式 str 而不是 enum

如果是 enum，第三方插件不能扩展（除非允许动态修改 enum）。如果是任意 str，拼写错误成本极低。注册式 str（`CapabilityName`）是折衷：

- 第三方插件用 `CapabilityName.register(name, declared_by=...)` 自由扩展
- 但 `CapabilityName("typo_here")` 在构造时立即抛 `UnknownCapabilityError`
- 同 owner 重注册幂等；跨 owner 同名抛冲突

详见 [registered-strings](../05-advanced/registered-strings.md)。

### 为什么 trace 是事件而不是日志

`logger.info(...)` 的 callback 没有 trace_id，串不起多插件链路；每条日志独立写，事后聚合昂贵。Trace 走 `bus.publish("trace.span", TraceSpan(...))`，订阅者按需写 sink（JSONL / OTel / 自定义），核心不知道也不关心。

更深层的原因：trace 是诊断 MutsukiBot 业务的**主入口**——出问题第一件事是看 trace，不是 log。把它做成结构化事件让"业务理解 trace"成为可能（写一个插件订阅 trace.span 自动检测异常 pattern）。

### 为什么 RefPayload 是契约层标记而不是基类

`RefPayload[T]` 不是要让你"继承它"，而是要让 codec / trace / audit 看到这个字段类型时，**知道行为约束**——拒绝序列化 handle 部分，只落 descriptor。把它做成基类会污染领域类型层级；做成字段标记则是非侵入的。

## 如何应用这些规则

写代码时遇到选择，按顺序问：

1. 这个能力属于 Agent 自身（`core`），还是属于插件？默认推到插件。
2. 我加的副作用登记到 scope 了吗？
3. 我添加的命令同时生成 LLM tool schema 了吗？
4. 我处理错误时是 raise 字符串异常，还是构造 `Error`？
5. 我用了 `time.time()` / `uuid.uuid4()` / `random.*` 吗？应当从 `ctx.*` 拿。
6. 我引入的外部协议放在 transport plugin 还是渗到 core？

如果某条规则在你的场景下需要松动，先在 plans / 进行讨论 —— 不要先写代码再补讨论。
