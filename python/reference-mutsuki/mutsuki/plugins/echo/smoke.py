"""冒烟入口 —— 端到端跑 echo 闭环（Plugin + Endpoint）。

``InMemoryEndpointPlugin`` 作为 reference plugin 与 ``EchoPlugin`` 一并装载。
Agent 显式声明
``accepts=(Scopes.IM_TEXT.to_rule(),)`` 以满足 [AGENTS.md hard rule #13](../../../AGENTS.md)。

额外断言：outbox message 的 ``source.source_id == "inmemory:default"``，
验证出站消息保留入站 transport 标识。

用法::

    python -m mutsuki.plugins.echo.smoke
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from tempfile import gettempdir

from mutsuki.contracts.ids import AgentId
from mutsuki.contracts.lifecycle import LifecyclePhase
from mutsuki.core.agent import Agent
from mutsuki.core.loader import PluginLoader
from mutsuki.observability import JsonlTraceWriter
from mutsuki.plugins.echo import EchoPlugin
from mutsuki.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsuki.runtime import NanoIdGen, SeededRng, SystemClock
from mutsuki.runtime.scheduler import AgentScheduler
from mutsuki_ext.command import TextCommandRouterPlugin
from mutsuki_ext.im import ChannelRef, IMScopes


async def main() -> None:
    agent = Agent(
        agent_id=AgentId("smoke-agent"),
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(seed=0),
        accepts=(IMScopes.TEXT.to_rule(),),
    )
    trace_path = Path(gettempdir()) / "mutsuki-echo-smoke.jsonl"
    writer = JsonlTraceWriter(trace_path)
    writer.attach(agent.bus)

    loader = PluginLoader(
        allow={EchoPlugin.id, InMemoryEndpointPlugin.id, TextCommandRouterPlugin.id}
    )
    await loader.load_into(
        agent, [InMemoryEndpointPlugin, TextCommandRouterPlugin, EchoPlugin]
    )

    print(f"[smoke] agent {agent.agent_id} phase={agent.phase}")
    print(f"[smoke] loaded plugins: {[p.plugin.id for p in agent.plugins]}")
    print(f"[smoke] dispatcher ops: {[op.op_id for op in agent.dispatch.list_operations()]}")
    print(f"[smoke] dispatcher sources: {[s.source_id for s in agent.dispatch.list_sources()]}")

    scheduler = AgentScheduler(agent)
    await scheduler.start()
    print(f"[smoke] phase={agent.phase}")

    inmem = next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    await inmem.send_text("echo hello")
    await asyncio.sleep(0.3)
    msgs = await inmem.drain_outbox(timeout=0.5)
    for m in msgs:
        print(f"[smoke] outbox -> {m.text!r}  source={m.source}")
        # v0.2 修复点：source.source_id 必须保留入站 transport 标识，
        # 不再是 v0.1 的硬编码 "agent"。
        assert isinstance(m.source, ChannelRef)
        assert m.source.source_id == "inmemory:default", (
            f"出站 source_id 期望 'inmemory:default'，实际 {m.source.source_id!r}"
        )

    await scheduler.stop()
    await loader.unload_from(agent)
    writer.detach()
    print(f"[smoke] phase={agent.phase}; trace at {trace_path}")
    assert agent.phase == LifecyclePhase.STOP


if __name__ == "__main__":
    asyncio.run(main())
