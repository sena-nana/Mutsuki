"""AgentRegistry —— 多 Agent 广播与路由筛选。"""

from __future__ import annotations

import pytest

from mutsukibot.contracts import Scopes
from mutsukibot.contracts.ids import AgentId
from mutsukibot.contracts.message import Message
from mutsukibot.core.agent import Agent
from mutsukibot.core.agent_registry import AgentRegistry
from mutsukibot.core.loader import PluginLoader
from mutsukibot.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock


def _new_agent(*, agent_id: str, accepts=()) -> Agent:
    return Agent(
        agent_id=AgentId(agent_id),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=accepts,
    )


@pytest.mark.asyncio
async def test_publish_broadcasts_to_other_accepting_agents_even_if_source_isnt() -> None:
    """source agent 不接收 envelope 时，仍应广播到其他匹配 Agent。"""
    AgentRegistry.clear()
    try:
        source_agent = _new_agent(agent_id="source", accepts=())
        target_agent = _new_agent(
            agent_id="target", accepts=(Scopes.IM_TEXT.to_rule(),)
        )

        loader = PluginLoader(allow={InMemoryEndpointPlugin.id})
        await loader.load_into(source_agent, [InMemoryEndpointPlugin])
        inmem = next(
            p.plugin
            for p in source_agent.plugins
            if isinstance(p.plugin, InMemoryEndpointPlugin)
        )

        await inmem.send_text("hello registry")

        assert source_agent.inbox.empty()
        assert target_agent.inbox.qsize() == 1
        item = await target_agent.inbox.get()
        assert isinstance(item, Message)
        assert item.text == "hello registry"
    finally:
        AgentRegistry.clear()
