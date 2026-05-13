"""Agent.make_context 在无插件时不应泄漏 PluginScope。"""

from __future__ import annotations

import pytest

from nanobot.contracts.ids import AgentId
from nanobot.core.agent import Agent
from nanobot.runtime import DeterministicIdGen, SeededRng, SystemClock


def _bare_agent() -> Agent:
    return Agent(
        agent_id=AgentId("ctx-test"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
    )


def test_make_context_no_plugins_reuses_single_scope() -> None:
    """无插件场景下，多次 make_context 必须返回同一个 fallback scope。"""
    agent = _bare_agent()
    ctx_a = agent.make_context()
    ctx_b = agent.make_context()
    assert ctx_a.scope is ctx_b.scope
    assert agent._agent_scope is ctx_a.scope


@pytest.mark.asyncio
async def test_close_agent_scope_releases_fallback() -> None:
    agent = _bare_agent()
    ctx = agent.make_context()
    scope = ctx.scope
    await agent.close_agent_scope()
    assert scope.closed
    assert agent._agent_scope is None


@pytest.mark.asyncio
async def test_close_agent_scope_idempotent_when_unused() -> None:
    """从未访问过 fallback scope 时，close_agent_scope 应静默成功。"""
    agent = _bare_agent()
    await agent.close_agent_scope()
    assert agent._agent_scope is None
