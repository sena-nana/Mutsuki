"""v0.3: explicit cross-agent Operation invoke."""

from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

from mutsukibot import Capability, Caps, Perms, Plugin, command
from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.event import TraceSpan
from mutsukibot.contracts.ids import AgentId
from mutsukibot.core.agent import Agent
from mutsukibot.core.agent_registry import AgentRegistry
from mutsukibot.core.dispatcher import OperationInvokeError
from mutsukibot.core.loader import PluginLoader
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsukibot.testing.contract_kit import assert_cross_agent_trace_chain


class _Conf(msgspec.Struct, kw_only=True):
    pass


class _RemoteMathPlugin(Plugin[_Conf]):
    id: ClassVar[str] = "test-remote-math"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    Config = _Conf

    @command(perms=Perms.PUBLIC)
    async def add(self, left: int, right: int) -> int:
        return left + right


def _agent(agent_id: str, *, id_seed: int = 0) -> Agent:
    return Agent(
        agent_id=AgentId(agent_id),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(seed=id_seed),
        rng=SeededRng(seed=0),
    )


@pytest.mark.asyncio
async def test_invoke_in_agent_calls_target_agent_operation() -> None:
    AgentRegistry.clear()
    caller = _agent("caller")
    target = _agent("target", id_seed=100)
    loader = PluginLoader(allow={_RemoteMathPlugin.id})
    await loader.load_into(target, [_RemoteMathPlugin])

    try:
        result = await caller.dispatch.invoke_in_agent(
            "target",
            "test-remote-math.add",
            {"left": 2, "right": 3},
            ctx=caller.make_context(),
        )
        assert result == 5
    finally:
        await loader.unload_from(target)
        AgentRegistry.clear()


@pytest.mark.asyncio
async def test_invoke_in_agent_links_caller_and_target_trace_spans() -> None:
    AgentRegistry.clear()
    caller = _agent("caller")
    target = _agent("target", id_seed=100)
    loader = PluginLoader(allow={_RemoteMathPlugin.id})
    await loader.load_into(target, [_RemoteMathPlugin])

    caller_spans: list[TraceSpan] = []
    target_spans: list[TraceSpan] = []

    async def collect_caller(payload: object) -> None:
        caller_spans.append(payload)  # type: ignore[arg-type]

    async def collect_target(payload: object) -> None:
        target_spans.append(payload)  # type: ignore[arg-type]

    caller.bus.subscribe("trace.span", collect_caller, direct=True)
    target.bus.subscribe("trace.span", collect_target, direct=True)

    try:
        result = await caller.dispatch.invoke_in_agent(
            "target",
            "test-remote-math.add",
            {"left": 2, "right": 3},
            ctx=caller.make_context(),
        )
        assert result == 5
        link = assert_cross_agent_trace_chain(caller_spans + target_spans)
        assert link.target.span.attributes["agent_id"] == "target"
        assert link.caller.span.attributes["target_agent_id"] == "target"
    finally:
        await loader.unload_from(target)
        AgentRegistry.clear()


@pytest.mark.asyncio
async def test_invoke_in_agent_unknown_target_is_structured_error() -> None:
    AgentRegistry.clear()
    caller = _agent("caller")

    with pytest.raises(OperationInvokeError) as exc:
        await caller.dispatch.invoke_in_agent(
            "missing",
            "test-remote-math.add",
            {"left": 2, "right": 3},
            ctx=caller.make_context(),
        )

    assert exc.value.error.code == Errs.AGENT_NOT_FOUND
    assert exc.value.error.evidence["agent_id"] == "missing"
    AgentRegistry.clear()
