"""Plugin.consumes —— envelope 二次分发到 plugin.on_envelope。

验证 D3（plugin.consumes 声明位）与共享 envelope consumer fan-out 逻辑：

* consumes=() 的 plugin 不收 envelope
* ScopeRule 匹配的 plugin 收到 on_envelope 调用
* 多 plugin 同时匹配 → 全部调用（fan-out）
* on_envelope 抛错隔离（不影响其他 plugin / 后续 envelope）
"""

from __future__ import annotations

import asyncio
from typing import ClassVar

import msgspec
import pytest

from mutsukicore import Capability, Caps, Plugin
from mutsukicore.contracts import (
    BySchema,
    BySourceKind,
    Envelope,
    ScopeRule,
    Scopes,
    SourceKinds,
)
from mutsukicore.contracts.event import SpanStatus, TraceSpan
from mutsukicore.contracts.ids import AgentId
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader
from mutsukicore.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukicore.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsukicore.runtime.scheduler import AgentScheduler
from mutsukicore_ext.im import Message


class _Conf(msgspec.Struct, kw_only=True):
    pass


class _RecorderPlugin(Plugin[_Conf]):
    """收到的 envelope 写入 class-level list；测试逐项断言。"""

    id: ClassVar[str] = "test-recorder"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    consumes: ClassVar[tuple[ScopeRule, ...]] = (
        BySchema("mutsukicore.message") & BySourceKind(SourceKinds.IM),
    )
    Config = _Conf
    received: ClassVar[list[Envelope]] = []

    async def on_envelope(self, envelope: Envelope) -> None:
        type(self).received.append(envelope)


class _CrashConsumerPlugin(Plugin[_Conf]):
    """on_envelope 抛错；用于验证错误隔离。"""

    id: ClassVar[str] = "test-crashconsumer"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    consumes: ClassVar[tuple[ScopeRule, ...]] = (Scopes.IM_ANY.to_rule(),)
    Config = _Conf

    async def on_envelope(self, envelope: Envelope) -> None:
        raise RuntimeError("consumer boom")


class _SilentPlugin(Plugin[_Conf]):
    """consumes=() —— 不接收任何 envelope。"""

    id: ClassVar[str] = "test-silent"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    Config = _Conf
    called: ClassVar[bool] = False

    async def on_envelope(self, envelope: Envelope) -> None:
        type(self).called = True


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("consumes-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


def _get_inmem(agent: Agent) -> InMemoryEndpointPlugin:
    return next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, InMemoryEndpointPlugin)
    )


@pytest.mark.asyncio
async def test_matching_consumes_receives_envelope() -> None:
    _RecorderPlugin.received = []
    agent = _new_agent()
    loader = PluginLoader(allow={InMemoryEndpointPlugin.id, _RecorderPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, _RecorderPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    inmem = _get_inmem(agent)
    await inmem.send_text("hello world")
    await asyncio.sleep(0.2)

    assert len(_RecorderPlugin.received) == 1
    received = _RecorderPlugin.received[0]
    assert isinstance(received, Message)
    assert received.text == "hello world"

    await scheduler.stop()
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_silent_plugin_with_empty_consumes_is_skipped() -> None:
    _SilentPlugin.called = False
    agent = _new_agent()
    loader = PluginLoader(allow={InMemoryEndpointPlugin.id, _SilentPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, _SilentPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    inmem = _get_inmem(agent)
    await inmem.send_text("anything")
    await asyncio.sleep(0.2)

    assert _SilentPlugin.called is False  # consumes=() 路径未触发

    await scheduler.stop()
    await loader.unload_from(agent)


@pytest.mark.asyncio
async def test_consumer_exception_is_isolated_and_traced() -> None:
    """on_envelope 抛错时不应中断后续 envelope，且 trace span 标记 ERROR。"""
    _RecorderPlugin.received = []
    agent = _new_agent()
    loader = PluginLoader(
        allow={
            InMemoryEndpointPlugin.id,
            _CrashConsumerPlugin.id,
            _RecorderPlugin.id,
        }
    )
    await loader.load_into(
        agent, [InMemoryEndpointPlugin, _CrashConsumerPlugin, _RecorderPlugin]
    )
    scheduler = AgentScheduler(agent)
    spans: list[TraceSpan] = []

    async def _on_span(payload: object) -> None:
        if isinstance(payload, TraceSpan):
            spans.append(payload)

    agent.bus.subscribe("trace.span", _on_span)
    await scheduler.start()

    inmem = _get_inmem(agent)
    await inmem.send_text("first")
    await asyncio.sleep(0.1)
    await inmem.send_text("second")
    await asyncio.sleep(0.2)

    # 错误隔离：recorder plugin 仍收到两条
    assert len(_RecorderPlugin.received) == 2

    # crash consumer 的两次失败均被 trace span 记录为 ERROR 状态
    crash_spans = [
        s for s in spans
        if s.name == f"plugin.{_CrashConsumerPlugin.id}.on_envelope"
    ]
    assert len(crash_spans) == 2
    assert all(s.status == SpanStatus.ERROR for s in crash_spans)
    assert all(s.attributes.get("exception_type") == "RuntimeError" for s in crash_spans)

    await scheduler.stop()
    await loader.unload_from(agent)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
