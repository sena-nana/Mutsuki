"""In-memory IM endpoint reference plugin —— 测试驱动入口。

它在 ``on_load`` 中通过 dispatcher 注册一个 IM kind 的 Source
（``"inmemory:default"``），并暴露 ``send_text`` / ``drain_outbox`` 两个
测试驱动方法。

测试用法::

    loader = PluginLoader(allow={EchoPlugin.id, InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, EchoPlugin])
    inmem = next(
        p.plugin for p in agent.plugins
        if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    await inmem.send_text("echo hello")
    msgs = await inmem.drain_outbox(timeout=0.5)

注意 Agent 必须显式声明 ``accepts=(Scopes.IM_TEXT.to_rule(),)`` 否则 envelope
会被 dispatcher 按 hard rule #13 拒绝（详 contracts §17）。
"""

from __future__ import annotations

import asyncio
from typing import ClassVar

import msgspec

from mutsukibot import Capability, Caps, Plugin
from mutsukibot.contracts import MessageId, SourceDescriptor
from mutsukibot_ext.im import (
    ChannelRef,
    ContentKind,
    ContentPart,
    IMCaps,
    IMSourceKinds,
    Message,
)

# hard-coded source 描述符 —— 必须在类级声明（D9b 静态契约），同时运行时
# on_load 中用同一对象注册，保证 declared/registered 一致。
_INMEMORY_SOURCE = SourceDescriptor(
    source_id="inmemory:default",
    kind=IMSourceKinds.IM,
    capabilities=(IMCaps.TEXT,),
    description="In-memory IM transport for tests and smoke scripts.",
)


class _InMemoryConfig(msgspec.Struct, kw_only=True):
    channel: str = "test"
    user: str = "test-user"


class InMemoryEndpointPlugin(Plugin[_InMemoryConfig]):
    """进程内 IM endpoint —— 测试与冒烟脚本的标准 transport plugin。"""

    id: ClassVar[str] = "mutsukibot-inmemory-endpoint"
    version: ClassVar[str] = "0.2.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    # D9b 静态声明：plugin 会注册的 Source（dispatcher 校验）
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = (_INMEMORY_SOURCE,)
    Config = _InMemoryConfig

    async def on_load(self) -> None:
        # 注册 Source —— 让本 plugin 有合法身份去 dispatch.publish envelope。
        # 反注册回调由 dispatcher 自动挂到 self.scope，plugin 卸载时清理。
        self.agent.dispatch.register_source(
            _INMEMORY_SOURCE,
            plugin_scope=self.scope,
            plugin_id=self.id,
        )

    async def send_text(self, text: str) -> Message:
        """构造一条 IM 消息并通过 dispatcher 发布到 Agent inbox。"""
        msg = Message(
            id=MessageId(self.agent.id_gen.next("msg")),
            timestamp=self.agent.clock.now(),
            source=ChannelRef(
                source_id=_INMEMORY_SOURCE.source_id,
                kind=IMSourceKinds.IM,
                channel_id=self.config.channel,
                user_id=self.config.user,
            ),
            payload_schema_id="mutsukibot.message",
            capabilities_required=(IMCaps.TEXT,),
            parts=(ContentPart(kind=ContentKind.TEXT, text=text),),
        )
        await self.agent.dispatch.publish(msg)
        return msg

    async def drain_outbox(self, timeout: float = 1.0) -> list[Message]:
        """在 ``timeout`` 秒内读尽所有可用 outbox 消息。"""
        out: list[Message] = []
        loop = asyncio.get_event_loop()
        deadline = loop.time() + timeout
        while loop.time() < deadline:
            try:
                msg = await asyncio.wait_for(self.agent.outbox.get(), timeout=0.05)
                if isinstance(msg, Message):
                    out.append(msg)
            except TimeoutError:
                if out:
                    return out
        return out


__all__ = ["InMemoryEndpointPlugin"]
