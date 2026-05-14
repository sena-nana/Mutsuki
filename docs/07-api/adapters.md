# API · `mutsukibot.adapters`

外部协议 → 内部 `Message` 的翻译层。

来源：[mutsukibot/adapters/__init__.py](../../mutsukibot/adapters/__init__.py)。

## 模块地图

| 模块 | 公开符号 |
|---|---|
| [`base`](#base) | `Adapter` / `AdapterCapability` |
| [`inmemory`](#inmemory) | `InMemoryAdapter` |

详见 [写一个 Adapter](../06-developer/writing-adapter.md)。

---

## base

[base.py](../../mutsukibot/adapters/base.py)

```python
class AdapterCapability(StrEnum):
    TEXT, IMAGE, AUDIO, FILE, MARKDOWN, CARD, REACTION, TYPING

class Adapter(ABC):
    adapter_id: ClassVar[str] = ""
    supports: ClassVar[tuple[AdapterCapability, ...]] = ()

    def __init_subclass__(cls, **kwargs) -> None
        # adapter_id 非空时自动注册到 AdapterRegistry

    @abstractmethod
    async def deliver(self, agent: Agent, message: Message) -> None

    @abstractmethod
    async def receive(self, agent: Agent) -> Message | None
```

## inmemory

[inmemory.py](../../mutsukibot/adapters/inmemory.py)

```python
class InMemoryAdapter(Adapter):
    adapter_id: ClassVar[str] = "inmemory"
    supports: ClassVar[tuple[AdapterCapability, ...]] = (AdapterCapability.TEXT,)

    def __init__(self, *, channel: str = "test", user: str = "test-user")

    async def send_text(self, agent: Agent, text: str) -> Message
        # 构造 TEXT Message 并 put 到 agent.inbox

    async def deliver(self, agent: Agent, message: Message) -> None
        # 把 message put 到 agent.outbox

    async def receive(self, agent: Agent) -> Message | None
        # 从 agent.outbox 拉一条，0.1s 超时则 None

    async def drain_outbox(self, agent: Agent, timeout: float = 1.0) -> list[Message]
        # 在 timeout 内尽量多取
```

`InMemoryAdapter` 是测试 / 冒烟入口。生产场景用其他 adapter（CLI / OneBot / WS …），核心仓库当前不内置真实平台 adapter（v0.2 候选）。
