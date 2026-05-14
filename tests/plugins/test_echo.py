"""Echo 插件：完整生命周期、命令 + tool schema、热重载安全。"""

from __future__ import annotations

import asyncio

import pytest

from nanobot.adapters import InMemoryAdapter
from nanobot.contracts.ids import AgentId
from nanobot.contracts.lifecycle import LifecyclePhase
from nanobot.core.agent import Agent
from nanobot.core.loader import PluginLoader
from nanobot.plugins.echo import EchoPlugin
from nanobot.runtime import DeterministicIdGen, SeededRng, SystemClock
from nanobot.runtime.scheduler import AgentScheduler


def _new_agent() -> Agent:
    return Agent(
        agent_id=AgentId("test-agent"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
    )


@pytest.mark.asyncio
async def test_full_lifecycle() -> None:
    agent = _new_agent()

    loader = PluginLoader(allow={EchoPlugin.id})
    await loader.load_into(agent, [EchoPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()
    assert agent.phase == LifecyclePhase.AWAKE

    adapter = InMemoryAdapter()
    await adapter.send_text(agent, "echo hello")
    await asyncio.sleep(0.3)
    msgs = await adapter.drain_outbox(agent, timeout=0.5)
    assert msgs, "应至少有一条 outbox 消息"
    assert any("echo: hello" in m.text for m in msgs)

    await scheduler.stop()
    await loader.unload_from(agent)
    assert agent.phase == LifecyclePhase.STOP


@pytest.mark.asyncio
async def test_command_path_and_tool_schema_share_signature() -> None:
    spec = EchoPlugin.__commands__[0]
    assert spec.name == "echo"
    assert spec.is_tool is True
    assert spec.description == "回显输入文本。"
    assert spec.parameters_schema["properties"]["text"]["type"] == "string"
    assert spec.parameters_schema["properties"]["count"]["minimum"] == 1
    assert spec.parameters_schema["properties"]["count"]["maximum"] == 10
    assert (
        spec.parameters_schema["properties"]["text"]["description"]
        == "要回显的文本。"
    )


@pytest.mark.asyncio
async def test_hot_reload_no_leaks() -> None:
    """加载 + 卸载 echo 插件 100 次；如有泄漏会以 HandleLeakError 暴露。"""
    agent = _new_agent()
    loader = PluginLoader(allow={EchoPlugin.id})
    for _ in range(100):
        await loader.load_into(agent, [EchoPlugin])
        await loader.unload_from(agent)
    assert agent.plugins == []
