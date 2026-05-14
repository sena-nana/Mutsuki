# Agent 与生命周期

## 这是什么

`Agent` 是 MutsukiBot 里的**一等运行时实体** —— 不是会话、不是 LLM 调用、不是平台连接，而是带身份的常驻对象。它有自己的 `agent_id`、自己的事件总线、自己的入站 / 出站队列、自己的插件集合，以及一个独立的 tick 调度器（一个 `asyncio.Task`）。

代码：[mutsukibot/core/agent.py](../../mutsukibot/core/agent.py)。

## 解决什么问题

传统 bot 框架里，"bot" 通常是一个无状态的回调集合 + 一些会话级 state。MutsukiBot 要承载 Yume / mind-sim 这类需要长时间运行、有内在状态、要主动行动的 agent，会话语义不够 —— 必须有一个明确的、可以被 spawn / awake / sleep / stop 的对象。

把它做成一等公民有两个直接收益：

1. **生命周期可观察**。每个 Agent 有 `phase: LifecyclePhase`，外部代码可以判断「这个 agent 现在能不能接收消息」。
2. **资源边界清晰**。Agent 拥有自己的 `ServiceContainer`、`Bus`、`PluginScope`，卸载一个 Agent 等于卸载它持有的全部资源 —— 不会出现「卸载完还有定时器在跑」。

## 怎么工作

### Agent 数据形态

[agent.py:55-72](../../mutsukibot/core/agent.py#L55-L72) 是个普通 dataclass：

```python
@dataclass
class Agent:
    agent_id: AgentId
    clock: "Clock"
    id_gen: "IdGen"
    rng: "RNG"
    owner: str | None = None
    services: ServiceContainer = field(default_factory=ServiceContainer)
    bus: Bus = field(default_factory=Bus)
    lifespan: Lifespan = field(default_factory=Lifespan)
    inbox: asyncio.Queue[Message] = field(default_factory=asyncio.Queue)
    outbox: asyncio.Queue[Message] = field(default_factory=asyncio.Queue)
    phase: LifecyclePhase = LifecyclePhase.SPAWN
    plugins: list[_LoadedPlugin] = field(default_factory=list)
    _agent_scope: PluginScope | None = field(default=None, repr=False)
    _command_index: dict[str, CommandTarget] = field(default_factory=dict, repr=False)
```

注意：`clock` / `id_gen` / `rng` 是**外部注入的**，不是默认构造的。这是 hard rule #9 的体现——决定性时间与 ID 由 runtime 注入，不允许 Agent 自己 `time.time()`。

### 生命周期阶段

`LifecyclePhase`（[lifecycle.py:6](../../mutsukibot/contracts/lifecycle.py#L6)）只有四个值，按顺序流转：

| 阶段 | 含义 | 触发点 |
|---|---|---|
| `SPAWN` | 身份创建完毕，调度尚未开始 | `Agent(...)` 构造完成时 |
| `AWAKE` | 调度器启动，开始接收命令 | `AgentScheduler.start()` |
| `SLEEP` | 调度器暂停 | `AgentScheduler.stop()` 早段 |
| `STOP` | 完全停止，插件已卸载 | `AgentScheduler.stop()` 末段 |

每个阶段都触发 [lifespan.py:32](../../mutsukibot/core/lifespan.py#L32) 的 `Lifespan.fire()`，按声明顺序运行 `on_spawn` / `on_awake`，按反序运行 `on_sleep` / `on_stop`（LIFO 退栈，模拟资源 acquire / release 的对称性）。

### 命令索引：O(1) 路由

加载插件时 [agent.py:98-107](../../mutsukibot/core/agent.py#L98-L107) 会扫描插件类的 `__command_markers__`（这个属性由 `PluginMeta` 在类定义时填充，详见 [插件定义](plugin-definition.md)），把每个命令的 `attr_name` 与函数名都登记到 `_command_index: dict[str, CommandTarget]`：

```python
def attach_plugin(self, plugin: "Plugin", scope: PluginScope) -> None:
    self.plugins.append(_LoadedPlugin(plugin, scope))
    markers: dict[str, "_CommandMarker"] = plugin.__class__.__command_markers__
    for attr_name, marker in markers.items():
        target = CommandTarget(
            plugin=plugin, attr_name=attr_name, scope=scope, marker=marker
        )
        self._command_index.setdefault(attr_name, target)
        self._command_index.setdefault(marker.func.__name__, target)
```

调度器分发命令时只做一次 `dict.get` 查表（[scheduler.py:88](../../mutsukibot/runtime/scheduler.py#L88) 的 `find_command`），不需要每条消息都 `inspect`。`setdefault` 保证先注册的不会被覆盖。

### Agent 自有 fallback scope

[agent.py:74-96](../../mutsukibot/core/agent.py#L74-L96) 的 `make_context()` 创建上下文时，会用一个**懒初始化**的 `_agent_scope`：

```python
if self._agent_scope is None:
    self._agent_scope = PluginScope(self.agent_id)
scope = self._agent_scope
```

为什么这么做：早期版本里，没有命令上下文时（如 `lifespan.fire`）我们直接借用第一个加载插件的 scope —— 结果那个插件被卸载时把 agent 的 lifespan 钩子上下文也带走了。现在 fallback scope 与任何插件解耦，由 `AgentScheduler.stop()` 显式 `await self.agent.close_agent_scope()` 关闭（[scheduler.py:65](../../mutsukibot/runtime/scheduler.py#L65)）。

命令路由路径不会用 fallback scope —— 调度器会显式把上下文里的 scope 替换为命令所属插件的 scope（[scheduler.py:106](../../mutsukibot/runtime/scheduler.py#L106)），这样命令副作用就跟着插件走。

## 用法示例

构造一个 Agent 是赤裸的：

```python
from mutsukibot.contracts.ids import AgentId
from mutsukibot.core.agent import Agent
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock

agent = Agent(
    agent_id=AgentId("smoke-agent"),
    clock=SystemClock(),
    id_gen=DeterministicIdGen(),  # 测试场景；生产用 NanoIdGen
    rng=SeededRng(seed=0),
)
```

接下来通常的流程：

1. `await PluginLoader().load_into(agent, [...])` 装载插件
2. `await AgentScheduler(agent).start()` 启动 tick 循环
3. 通过 adapter 往 `agent.inbox` 投消息，从 `agent.outbox` 取响应
4. `await scheduler.stop()` + `await loader.unload_from(agent)` 收尾

完整闭环在 [mutsukibot/plugins/echo/smoke.py](../../mutsukibot/plugins/echo/smoke.py)。

## 常见陷阱

- **不要绕开注入直接 `time.time()` / `uuid.uuid4()` / `random.random()`**。这是 hard rule #9，违反会让 `ManualClock` + `DeterministicIdGen` 的可重放测试失效。所有运行时来源都从 `AgentContext` 拿（详见 [AgentContext](agent-context.md)）。
- **不要直接复用 `agent.bus.subscribe(...)` 的返回值**。一定要把它登记到当前 scope（命令里是 `ctx.scope.add_subscription(unsub)`），否则插件卸载后订阅还在，会被 `HandleLeakError` 拒绝（详见 [PluginScope](plugin-scope.md)）。
- **`agent.phase` 由调度器维护，不要手动写**。Lifespan 钩子里读 phase 是安全的；写 phase 是 bug。
- **一个 Agent 只能由一个 `AgentScheduler` 驱动**。当前实现没做互斥 —— 由调用方约定。
