from __future__ import annotations

from typing import ClassVar

import msgspec

from mutsuki import Capability, Caps, Perms, Plugin
from mutsuki.contracts import AgentId, OperationDescriptor
from mutsuki.core.agent import Agent
from mutsuki.core.loader import PluginLoader
from mutsuki.runtime import DeterministicIdGen, SeededRng, SystemClock
from mutsuki.testing.benchmark import measure_dispatcher_invoke


class _BenchConfig(msgspec.Struct, kw_only=True):
    pass


_OP_NOOP = OperationDescriptor(
    op_id="bench:dispatcher.noop",
    name="noop",
    description="Benchmark no-op operation.",
    plugin_id="test-dispatcher-benchmark",
)


class _BenchPlugin(Plugin[_BenchConfig]):
    id: ClassVar[str] = "test-dispatcher-benchmark"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.READ_MESSAGE)]
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (_OP_NOOP,)
    Config = _BenchConfig

    async def on_load(self) -> None:
        self.agent.dispatch.register_operation(
            _OP_NOOP,
            handler=self._noop,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )

    async def _noop(self, _ctx, _payload):
        return "ok"


async def test_dispatcher_invoke_has_sub_ms_baseline() -> None:
    agent = Agent(
        agent_id=AgentId("bench-agent"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
    )
    loader = PluginLoader(allow={_BenchPlugin.id})
    await loader.load_into(agent, [_BenchPlugin])

    stats = await measure_dispatcher_invoke(
        agent.dispatch,
        "bench:dispatcher.noop",
        ctx=agent.make_context(),
        iterations=200,
    )

    assert stats.mean_ms < 1.0
    assert stats.p95_ms < 2.0

    await loader.unload_from(agent)
