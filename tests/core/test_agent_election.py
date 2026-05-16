"""v0.3: deterministic accepting-agent selection."""

from __future__ import annotations

from mutsukibot.contracts import (
    Caps,
    ChannelRef,
    ContentKind,
    ContentPart,
    Message,
    MessageId,
    Scopes,
    SourceKinds,
)
from mutsukibot.contracts.ids import AgentId
from mutsukibot.core.agent import Agent
from mutsukibot.core.agent_registry import AgentRegistry
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock


def _agent(agent_id: str, *, priority: int) -> Agent:
    return Agent(
        agent_id=AgentId(agent_id),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
        priority=priority,
    )


def _message() -> Message:
    return Message(
        id=MessageId("m1"),
        timestamp=0.0,
        source=ChannelRef(
            source_id="inmemory:default",
            kind=SourceKinds.IM,
            channel_id="c",
        ),
        payload_schema_id="mutsukibot.message",
        capabilities_required=(Caps.IM_TEXT,),
        parts=(ContentPart(kind=ContentKind.TEXT, text="hi"),),
    )


def test_select_accepting_prefers_highest_priority_then_agent_id() -> None:
    AgentRegistry.clear()
    try:
        _agent("zeta", priority=10)
        alpha = _agent("alpha", priority=10)
        _agent("middle", priority=1)

        selected = AgentRegistry.select_accepting(_message())

        assert selected is alpha
    finally:
        AgentRegistry.clear()


def test_rank_accepting_returns_stable_order() -> None:
    AgentRegistry.clear()
    try:
        agents = (
            _agent("b", priority=1),
            _agent("a", priority=1),
            _agent("c", priority=2),
        )

        ranked = AgentRegistry.rank_accepting(_message())

        assert [str(agent.agent_id) for agent in ranked] == ["c", "a", "b"]
        assert agents[0].agent_id == "b"
    finally:
        AgentRegistry.clear()
