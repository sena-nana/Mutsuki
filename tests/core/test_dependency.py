"""Dependent + Param 的认领/求解行为。"""

from __future__ import annotations

from typing import Annotated

import pytest

from mutsukibot.contracts.ids import AgentId, SpanId, TraceId
from mutsukibot.contracts.plugin import Arg
from mutsukibot.core.bus import Bus
from mutsukibot.core.container import ServiceContainer
from mutsukibot.core.context import AgentContext, TraceContext
from mutsukibot.core.dependency import Dependent, UnresolvedParameterError
from mutsukibot.core.scope import PluginScope
from mutsukibot.runtime import NanoIdGen, SeededRng, SystemClock


def _ctx() -> AgentContext:
    return AgentContext(
        agent_id=AgentId("a"),
        agent_owner=None,
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(0),
        services=ServiceContainer(),
        scope=PluginScope("p"),
        bus=Bus(),
        trace_ctx=TraceContext(trace_id=TraceId("t"), span_id=SpanId("s")),
    )


@pytest.mark.asyncio
async def test_ctx_param_resolves_agent_context() -> None:
    async def fn(ctx: AgentContext) -> str:
        return ctx.agent_id

    dep = Dependent.parse(fn)
    result = await dep.solve(_ctx())
    assert result == "a"


@pytest.mark.asyncio
async def test_arg_param_passes_through_extras() -> None:
    async def fn(ctx: AgentContext, text: str) -> str:
        return text.upper()

    dep = Dependent.parse(fn)
    result = await dep.solve(_ctx(), text="hello")
    assert result == "HELLO"


@pytest.mark.asyncio
async def test_annotated_constraints_dont_block_resolution() -> None:
    async def fn(ctx: AgentContext, n: Annotated[int, Arg(ge=1, le=10)] = 1) -> int:
        return n * 2

    dep = Dependent.parse(fn)
    assert await dep.solve(_ctx(), n=3) == 6


def test_unannotated_parameter_rejected() -> None:
    async def fn(ctx, text):  # type: ignore[no-untyped-def]
        return text

    with pytest.raises(UnresolvedParameterError):
        Dependent.parse(fn)
