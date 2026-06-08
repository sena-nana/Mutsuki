# AgentContext

## 这是什么

`AgentContext` 是**单次调用的运行时上下文**。插件命令的签名里以 `ctx: AgentContext` 形式接收，框架在分发命令时构造一个新实例并注入。

代码：[mutsukibot/core/context.py](../../mutsukibot/core/context.py)。

## 解决什么问题

插件需要时间、ID、随机数、订阅事件、解析服务、跟踪因果链 —— 如果让插件作者自己去 `import time, uuid, random`，三件事会立刻坏掉：

1. **测试不可重放**。每次 `uuid.uuid4()` 都给一个新值，写不出"输入相同 → 输出相同"的回归测试。
2. **跨域副作用泄漏**。订阅总线没绑 scope，卸载插件留下一堆活着的 handler。
3. **审计断链**。trace span 找不到 parent，看不出谁触发了谁。

`AgentContext` 把这些能力收敛成**唯一入口**，框架负责注入实现，插件只用接口。这是 [AGENTS.md](../../AGENTS.md) hard rule #9（决定性时间与 ID 由 runtime 注入）的执行点。

## 怎么工作

### 字段语义

[context.py:32-46](../../mutsukibot/core/context.py#L32-L46)：

```python
@dataclass(slots=True)
class AgentContext:
    agent_id: AgentId
    agent_owner: str | None
    clock: "Clock"
    id_gen: "IdGen"
    rng: "RNG"
    services: "ServiceContainer"
    scope: "PluginScope"
    bus: "Bus"
    dispatch: "Dispatcher"
    trace_ctx: TraceContext
    message: "Message | None" = None
    extras: dict[str, object] = field(default_factory=dict)
```

| 字段 | 用途 | 详见 |
|---|---|---|
| `agent_id` / `agent_owner` | 身份与归属 | [Agent 与生命周期](agent-and-lifecycle.md) |
| `clock` | 墙钟 / 单调时间 / async sleep | [Clock 协议](../06-developer/writing-runtime.md) |
| `id_gen` | 不透明 ID 生成 | 同上 |
| `rng` | 可重放随机源 | 同上 |
| `services` | 按 `(契约类型, name)` 解析服务 | [服务容器](service-container.md) |
| `scope` | 当前调用的资源生命周期作用域 | [PluginScope](plugin-scope.md) |
| `bus` | 进程内事件总线 | [事件总线](event-bus.md) |
| `dispatch` | Operation / Source 调用入口 | [插件 DAG 加载](../05-advanced/plugin-loader-dag.md) |
| `trace_ctx` | trace_id / span_id / parent_span_id | [Trace 与 Span](trace-and-span.md) |
| `message` | 触发本次调用的消息（命令路径必有；lifespan 钩子为 None） | [API · contracts.Message](../07-api/contracts.md#message) |
| `extras` | 插件可以临时塞 per-call 状态 | —— |

### TraceContext

```python
@dataclass(slots=True)
class TraceContext:
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None
```

`trace_id` 跨整个外部触发链路保持不变，`span_id` 标识当前这一跳，`parent_span_id` 指向调用方的 span。文本命令入口由 [`TextCommandRouterPlugin`](../../mutsukibot_ext/command/__init__.py) 构造命令调用 ctx；generic envelope consumer 由 [`dispatch_envelope_to_consumers`](../../mutsukibot/runtime/envelope_dispatch.py) 构造 consumer ctx。插件需要嵌套调用时应当继承当前 ctx 的 `trace_id`、把当前 `span_id` 作为子调用的 `parent_span_id`。

### 谁来构造它

主要路径：

1. **命令路由**：`TextCommandRouterPlugin` 解析文本后调用 `Agent.make_context(message=msg)`，再走 `ctx.dispatch.invoke(...)`。
2. **Envelope consumer**：scheduler 与 Rust/Python backend adapter 共用 `dispatch_envelope_to_consumers(...)`，按 `Plugin.consumes` fan-out 到 `plugin.on_envelope(...)`。
3. **生命周期钩子**：`Agent.make_context()` 使用 agent 自有 fallback scope，`message=None`。

这些路径都会**新建** `TraceContext` —— 没有 parent_span_id 即代表外部触发的根 span。

## 用法示例

最简命令：

```python
from mutsukibot import AgentContext, Plugin, command

class MyPlugin(Plugin[Config]):
    id = "my-plugin"
    version = "0.1.0"
    capabilities = []
    Config = MyConfig

    @command()
    async def now(self, ctx: AgentContext) -> str:
        """返回当前墙钟时间。"""
        ts = ctx.clock.now()
        new_id = ctx.id_gen.next("op")
        return f"{new_id} @ {ts}"
```

订阅事件并把 unsubscribe 注册进 scope：

```python
@command()
async def subscribe_demo(self, ctx: AgentContext) -> str:
    async def handler(payload: object) -> None:
        ...
    unsub = ctx.bus.subscribe("my-event", handler)
    ctx.scope.add_subscription(unsub)
    return "subscribed"
```

调用其他插件的 Operation：

```python
result = await ctx.dispatch.invoke(
    "backend:default.notify",
    {"message": "agent observed an external event"},
    ctx=ctx,
)
```

发布 trace 子 span（嵌套调用场景）：

```python
sub_span_id = SpanId(ctx.id_gen.next("span"))
sub_trace = TraceContext(
    trace_id=ctx.trace_ctx.trace_id,
    span_id=sub_span_id,
    parent_span_id=ctx.trace_ctx.span_id,
)
```

## 常见陷阱

- **`ctx.message is None` 是合法状态**。lifespan 钩子里收到的 ctx 没有触发消息。命令路径里它一定有值，但写防御代码时仍要判 None（pyright 会提醒）。
- **`extras` 不是跨调用 state**。它是 per-call 字典，命令返回后就被丢弃。要持久化状态用 `self.config` 或 `ctx.services.register(...)`。
- **不要把 `ctx` 缓存到 `self`**。它代表"这一次调用"的副作用域；下一次调用是新 ctx，scope / trace_ctx 都不一样。
- **`ctx.scope` 默认是 agent fallback scope**。Operation 执行与插件资源仍由注册时绑定的 `PluginScope` 负责回收；在命令或 consumer 内注册长期副作用时，优先使用当前插件自身的 `self.scope`。
