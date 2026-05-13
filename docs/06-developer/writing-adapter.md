# 写一个 Adapter

## 这是什么

Adapter 把外部协议（CLI 输入 / WS 帧 / OneBot / HTTP webhook ……）翻译成 NanoBot 内部 `Message` 契约，反之亦然。它**不**含业务逻辑——业务靠插件实现。

代码：

- 基类：[nanobot/adapters/base.py](../../nanobot/adapters/base.py)
- 参考实现：[nanobot/adapters/inmemory.py](../../nanobot/adapters/inmemory.py)

## 解决什么问题

[hard rule #11](../../AGENTS.md)：**双协议分离** —— 外部协议（OneBot / MCP / ChatCompletion 等）只能出现在 adapters / 桥接插件里，不得渗透 `core` / `contracts`。

理由：业务侧不该感知"这条消息是从 QQ 还是 Discord 来的" —— 一旦感知，业务实现就跟平台耦合，换平台要改业务。Adapter 把平台细节扁平化为 `Message`：业务侧只看通用结构。

## 怎么工作

### Adapter 抽象基类

[adapters/base.py:27-40](../../nanobot/adapters/base.py#L27-L40)：

```python
class Adapter(ABC):
    adapter_id: ClassVar[str] = ""
    supports: ClassVar[tuple[AdapterCapability, ...]] = ()

    def __init_subclass__(cls, **kwargs: object) -> None:
        super().__init_subclass__(**kwargs)
        if cls.adapter_id:
            AdapterRegistry.register(cls.adapter_id, cls)

    @abstractmethod
    async def deliver(self, agent: "Agent", message: "Message") -> None: ...

    @abstractmethod
    async def receive(self, agent: "Agent") -> "Message | None": ...
```

要点：

- **`adapter_id` 在类定义时自动注册**——通过 `__init_subclass__` 登记到 `AdapterRegistry`。`adapter_id` 留空（默认 `""`）则不登记，方便有抽象中间基类
- **`deliver` 把出站消息推给外部**（写 socket / 调 webhook / 落 stdout）
- **`receive` 拿一条入站消息**——返回 None 表示当前没有

### AdapterCapability

[adapters/base.py:16-24](../../nanobot/adapters/base.py#L16-L24)：

```python
class AdapterCapability(StrEnum):
    TEXT = "text"
    IMAGE = "image"
    AUDIO = "audio"
    FILE = "file"
    MARKDOWN = "markdown"
    CARD = "card"
    REACTION = "reaction"
    TYPING = "typing"
```

声明这个 adapter 能传输哪些 `ContentKind`。CLI adapter 只声 `TEXT`；OneBot / Discord 适配器可以声多个。当前 v0.1 没强制校验"消息部分必须落在 supports 集合里"，靠开发者自觉。

### InMemoryAdapter 参考

[adapters/inmemory.py](../../nanobot/adapters/inmemory.py) 是测试驱动入口，最小的可工作 adapter：

```python
class InMemoryAdapter(Adapter):
    adapter_id: ClassVar[str] = "inmemory"
    supports: ClassVar[tuple[AdapterCapability, ...]] = (AdapterCapability.TEXT,)

    def __init__(self, *, channel: str = "test", user: str = "test-user") -> None:
        self.channel = channel
        self.user = user

    async def send_text(self, agent: "Agent", text: str) -> Message:
        msg = Message(
            id=MessageId(agent.id_gen.next("msg")),
            timestamp=agent.clock.now(),
            source=ChannelRef(
                adapter_id=self.adapter_id,
                channel_id=self.channel,
                user_id=self.user,
            ),
            parts=(ContentPart(kind=ContentKind.TEXT, text=text),),
        )
        await agent.inbox.put(msg)
        return msg

    async def deliver(self, agent: "Agent", message: Message) -> None:
        await agent.outbox.put(message)

    async def receive(self, agent: "Agent") -> Message | None:
        try:
            return await asyncio.wait_for(agent.outbox.get(), timeout=0.1)
        except TimeoutError:
            return None

    async def drain_outbox(self, agent: "Agent", timeout: float = 1.0) -> list[Message]:
        ...
```

注意几点：

- `send_text` 是测试便利方法，不在 `Adapter` ABC 里
- `deliver` 在 InMemoryAdapter 里直接把 message 投回 outbox（用作 echo 测试）—— 真实 adapter 这里通常调外部 SDK
- `receive` 用 `asyncio.wait_for` 包 `outbox.get`——超时返 None；这跟 `AgentScheduler._loop` 的轮询模式一致

### 与 Agent 的关系

Adapter **不**自动绑定 Agent —— 它接收 `agent` 作为参数。这有两层好处：

1. 同一个 adapter 可以服务多个 Agent（一个 WS 连接路由多个 bot）
2. 测试时可以注入 mock agent

但代价是：adapter 自己不知道它服务哪个 agent，靠调用方传 —— 你的入站循环要自己路由。

### 注册路径

`__init_subclass__` 自动登记到 `AdapterRegistry`（[registry.py:66](../../nanobot/core/registry.py#L66)）。需要时 `AdapterRegistry.get("inmemory")` 拿类。

## 用法示例

写一个最小 CLI adapter（仅示意，不在仓库）：

```python
import asyncio
import sys
from typing import ClassVar
from nanobot.adapters.base import Adapter, AdapterCapability
from nanobot.contracts.ids import MessageId
from nanobot.contracts.message import ChannelRef, ContentKind, ContentPart, Message


class CliAdapter(Adapter):
    adapter_id: ClassVar[str] = "cli"
    supports: ClassVar[tuple[AdapterCapability, ...]] = (AdapterCapability.TEXT,)

    def __init__(self) -> None:
        self._stdin_queue: asyncio.Queue[str] = asyncio.Queue()
        self._reader_task: asyncio.Task | None = None

    async def start(self, agent) -> None:
        # 后台读 stdin，把每行投进 agent.inbox
        async def _reader() -> None:
            loop = asyncio.get_event_loop()
            while True:
                line = await loop.run_in_executor(None, sys.stdin.readline)
                if not line:
                    break
                msg = Message(
                    id=MessageId(agent.id_gen.next("msg")),
                    timestamp=agent.clock.now(),
                    source=ChannelRef(
                        adapter_id=self.adapter_id,
                        channel_id="stdin",
                        user_id="local",
                    ),
                    parts=(ContentPart(kind=ContentKind.TEXT, text=line.strip()),),
                )
                await agent.inbox.put(msg)
        self._reader_task = asyncio.create_task(_reader())

    async def stop(self) -> None:
        if self._reader_task is not None:
            self._reader_task.cancel()
            try:
                await self._reader_task
            except asyncio.CancelledError:
                pass

    async def deliver(self, agent, message: Message) -> None:
        print(message.text, flush=True)

    async def receive(self, agent) -> Message | None:
        return None  # 这个 adapter 用 reader_task 推到 inbox，不走 receive 拉模式
```

调用：

```python
adapter = CliAdapter()
await adapter.start(agent)
# ... agent.inbox 现在源源不断收到 stdin 的行 ...
# 业务命令产出的 outbox message 由谁送给 adapter？取决于编排——见下面的"陷阱"
```

## 常见陷阱

- **谁负责把 outbox message 送给 adapter？** v0.1 没有内置的 "outbox → adapter.deliver" 桥。调度器只把 message 投到 `agent.outbox`。要么：(a) 自己起个 task 不停从 outbox 取并 `await adapter.deliver(agent, msg)`；(b) 把 adapter 注册成订阅 `outbox` 类的事件（需要业务自己 publish）。InMemoryAdapter 在测试里用 `drain_outbox` 拉模式回避了这个问题。
- **`adapter_id` 必须 v0.1 全局唯一**——`AdapterRegistry` 冲突会抛 `RegistryConflictError`。命名约定：`onebot-v11`、`discord`、`cli`。
- **不要在 `__init__` 里启动后台 task**——`__init__` 没有 event loop 上下文（取决于何时构造）。把启动放在显式 `start(agent)` 方法里。
- **`supports` 不强制**。声了 `IMAGE` 但 `deliver` 收到 image 仍要自己处理或 raise。当前没有 "supports 不匹配 → reject" 的运行时检查。
- **Adapter 不自带 capability 声明**——它不是 plugin。如果 adapter 需要"占用网络"，靠业务封装它的桥接插件去声 `Caps.NETWORK_EGRESS`。
- **`receive` 与 `deliver` 抛异常时调用方处理方式由调用方决定**。当前 InMemoryAdapter 不抛；真实 adapter 应当把可恢复错误（连接断了）以结构化 `Error` 形式发回 bus（`adapter.error` 之类的事件名），而不是让异常逃出。
