"""Agent.accepts —— 路由筛选语义（contracts §17 / hard rule #13）+
出站 source.source_id 不丢的回归（修复 v0.1 scheduler.py:225 缺陷）。
"""

from __future__ import annotations

import asyncio

import pytest

from mutsukicore.contracts import (
    Caps,
    MessageId,
    Scopes,
    SourceKinds,
)
from mutsukicore.contracts.ids import AgentId
from mutsukicore.core.agent import Agent
from mutsukicore.core.dispatcher import OperationInvokeError
from mutsukicore.core.loader import PluginLoader
from mutsukicore.plugins.echo import EchoPlugin
from mutsukicore.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukicore.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsukicore.runtime.scheduler import AgentScheduler
from mutsukicore_ext.command import TextCommandRouterPlugin
from mutsukicore_ext.im import ChannelRef, ContentKind, ContentPart, Message


def _make_agent(*, accepts) -> Agent:
    return Agent(
        agent_id=AgentId("accepts-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=accepts,
    )


@pytest.mark.asyncio
async def test_empty_accepts_drops_envelope_silently() -> None:
    """Hard rule #13：空 accepts = 拒绝路由，不进 inbox。"""
    agent = _make_agent(accepts=())
    loader = PluginLoader(allow={InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin])

    inmem = next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    # publish 完成但 envelope 不会落到 inbox（被 dispatcher silently drop）
    await inmem.send_text("anything")
    # 确认 inbox 为空
    assert agent.inbox.empty()

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_matching_accepts_routes_to_inbox() -> None:
    """accepts 匹配时 envelope 进 inbox。"""
    agent = _make_agent(accepts=(Scopes.IM_TEXT.to_rule(),))
    loader = PluginLoader(allow={InMemoryEndpointPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin])

    inmem = next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    await inmem.send_text("hello")
    # inbox 应有一条 envelope
    assert agent.inbox.qsize() == 1
    item = await agent.inbox.get()
    assert isinstance(item, Message)
    assert item.text == "hello"

    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_outbound_message_preserves_source_id() -> None:
    """v0.1 缺陷修复回归：scheduler 出站 ChannelRef 不再硬编码 'agent'，
    而是从入站 message 复写 source.source_id。"""
    agent = _make_agent(accepts=(Scopes.IM_TEXT.to_rule(),))
    loader = PluginLoader(
        allow={EchoPlugin.id, InMemoryEndpointPlugin.id, TextCommandRouterPlugin.id}
    )
    await loader.load_into(agent, [InMemoryEndpointPlugin, TextCommandRouterPlugin, EchoPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    inmem = next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    await inmem.send_text("echo hi")
    await asyncio.sleep(0.2)
    msgs = await inmem.drain_outbox(timeout=0.5)
    assert msgs, "应至少有一条 outbox 消息"
    out = msgs[0]
    assert isinstance(out.source, ChannelRef)
    assert out.source.source_id == "inmemory:default"
    assert out.source.kind == SourceKinds.IM
    # v0.1 的 channel_id="agent_id" 硬编码也已修：复写自入站 channel
    assert out.source.channel_id == "test"

    await scheduler.stop()
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_publish_unregistered_source_errors() -> None:
    """直接构造未注册 source_id 的 envelope publish → SOURCE_UNREGISTERED。"""
    agent = _make_agent(accepts=(Scopes.IM_TEXT.to_rule(),))
    msg = Message(
        id=MessageId("m"),
        timestamp=0.0,
        source=ChannelRef(
            source_id="never:registered", kind=SourceKinds.IM, channel_id="c"
        ),
        payload_schema_id="mutsukicore.message",
        capabilities_required=(Caps.IM_TEXT,),
        parts=(ContentPart(kind=ContentKind.TEXT, text="x"),),
    )
    with pytest.raises(OperationInvokeError):
        await agent.dispatch.publish(msg)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
