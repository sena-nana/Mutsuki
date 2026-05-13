"""进程内 adapter —— v0.1 的测试驱动入口。

测试 / 冒烟脚本把它当作唯一传输使用，免去任何真实平台集成。``send_text``
把 Message 投到 Agent inbox；``read_outbox`` 抽取调度器产出的响应。
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, ClassVar

from nanobot.adapters.base import Adapter, AdapterCapability
from nanobot.contracts.ids import MessageId
from nanobot.contracts.message import ChannelRef, ContentKind, ContentPart, Message

if TYPE_CHECKING:
    from nanobot.core.agent import Agent


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
        """在 ``timeout`` 秒内读尽所有可用 outbox 消息。"""
        out: list[Message] = []
        deadline = asyncio.get_event_loop().time() + timeout
        while asyncio.get_event_loop().time() < deadline:
            try:
                msg = await asyncio.wait_for(agent.outbox.get(), timeout=0.05)
                out.append(msg)
            except TimeoutError:
                if out:
                    return out
        return out


__all__ = ["InMemoryAdapter"]
