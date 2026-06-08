"""AgentProfile / ExecutionStrategy 第一片契约。"""

from __future__ import annotations

from typing import Any, cast

import msgspec
import pytest

from mutsukibot.contracts import (
    AgentParticipation,
    AgentProfile,
    Caps,
    Decision,
    Envelope,
    EnvelopeId,
    Error,
    Errs,
    ExecutionStrategy,
    SchemaRegistry,
    Scopes,
    SideEffectPolicy,
    SourceKinds,
    StrategyResult,
    StrategyResultStatus,
)
from mutsukibot.contracts.ids import AgentId
from mutsukibot.core.agent import Agent
from mutsukibot.core.context import AgentContext
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsukibot_ext.im import ChannelRef, ContentKind, ContentPart, Message


def _message() -> Message:
    return Message(
        id=EnvelopeId("env-1"),
        timestamp=1.0,
        source=ChannelRef(
            source_id="inmemory:default",
            kind=SourceKinds.IM,
            channel_id="c",
        ),
        payload_schema_id="mutsukibot.message",
        capabilities_required=(Caps.IM_TEXT,),
        parts=(ContentPart(kind=ContentKind.TEXT, text="hi"),),
    )


class _Strategy:
    strategy_id: str = "tests.strategy"
    supported_profiles: tuple[str, ...] = ("tests.profile",)

    async def on_awake(self, ctx: AgentContext) -> None:
        _ = ctx

    async def on_input(self, ctx: AgentContext, envelope: Envelope) -> StrategyResult:
        _ = envelope
        await ctx.dispatch.invoke("tests.noop", {}, ctx=ctx)
        return StrategyResult(status=StrategyResultStatus.WAIT_INPUT)

    async def on_stop(self, ctx: AgentContext) -> None:
        _ = ctx

    async def next_step(self, ctx: AgentContext) -> StrategyResult:
        await ctx.dispatch.invoke("tests.noop", {}, ctx=ctx)
        return StrategyResult(status=StrategyResultStatus.CONTINUE)


def test_agent_profile_schema_is_registered_contract() -> None:
    assert SchemaRegistry.get("mutsukibot.agent_profile") is AgentProfile
    assert SchemaRegistry.get("mutsukibot.strategy_result") is StrategyResult


def test_observer_profile_defaults_to_read_only() -> None:
    profile = AgentProfile(
        profile_id="tests.observer",
        participation=AgentParticipation.OBSERVER,
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )

    assert profile.side_effect_policy == SideEffectPolicy.READ_ONLY
    assert profile.strategy_id == ""


def test_explicit_helper_without_accepts_rejects_external_input() -> None:
    profile = AgentProfile(
        profile_id="tests.helper",
        participation=AgentParticipation.EXPLICIT_HELPER,
    )
    agent = Agent(
        agent_id=AgentId("helper"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        profile=profile,
    )

    assert agent.participation == AgentParticipation.EXPLICIT_HELPER
    assert agent.effective_accepts == ()


def test_strategy_result_carries_decision_envelopes_and_error() -> None:
    envelope = _message()
    decision = Decision(
        id="decision-1",
        source="tests",
        route="strategy.on_input",
        payload=msgspec.Raw(b'{"next":"wait"}'),
        alternatives_considered=("continue",),
    )
    error = Error(
        code=Errs.OPERATION_NOT_FOUND,
        source="tests",
        route="strategy.on_input",
        evidence={"op_id": "missing"},
    )

    result = StrategyResult(
        status=StrategyResultStatus.FAILED,
        decision=decision,
        emitted=(envelope,),
        error=error,
    )
    default_result = StrategyResult(status=StrategyResultStatus.CONTINUE)

    assert result.decision is decision
    assert result.emitted == (envelope,)
    assert result.error is error
    assert default_result.emitted == ()
    assert isinstance(default_result.emitted, tuple)


@pytest.mark.asyncio
async def test_execution_strategy_shape_uses_dispatcher_invoke() -> None:
    strategy: ExecutionStrategy = _Strategy()
    calls: list[tuple[str, dict[str, Any]]] = []

    class _Dispatch:
        async def invoke(self, op_id: str, payload: dict[str, Any], *, ctx: Any) -> str:
            _ = ctx
            calls.append((op_id, payload))
            return "ok"

    class _Ctx:
        dispatch = _Dispatch()

    result = await strategy.on_input(cast(AgentContext, _Ctx()), _message())

    assert result.status == StrategyResultStatus.WAIT_INPUT
    assert calls == [("tests.noop", {})]
