"""冒烟入口 —— 端到端跑 v0.1 的 echo 闭环。

用法::

    python -m nanobot.plugins.echo.smoke
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from tempfile import gettempdir

from nanobot.adapters import InMemoryAdapter
from nanobot.contracts.ids import AgentId
from nanobot.contracts.lifecycle import LifecyclePhase
from nanobot.core.agent import Agent
from nanobot.core.loader import PluginLoader
from nanobot.observability import JsonlTraceWriter
from nanobot.plugins.echo import EchoPlugin
from nanobot.runtime import NanoIdGen, SeededRng, SystemClock
from nanobot.runtime.scheduler import AgentScheduler


async def main() -> None:
    agent = Agent(
        agent_id=AgentId("smoke-agent"),
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(seed=0),
    )
    trace_path = Path(gettempdir()) / "nanobot-echo-smoke.jsonl"
    writer = JsonlTraceWriter(trace_path)
    writer.attach(agent.bus)

    loader = PluginLoader(allow={EchoPlugin.id})
    await loader.load_into(agent, [EchoPlugin])

    print(f"[smoke] agent {agent.agent_id} phase={agent.phase}")
    print(f"[smoke] loaded plugins: {[p.plugin.id for p in agent.plugins]}")

    scheduler = AgentScheduler(agent)
    await scheduler.start()
    print(f"[smoke] phase={agent.phase}")

    adapter = InMemoryAdapter()
    await adapter.send_text(agent, "echo hello")
    await asyncio.sleep(0.3)
    msgs = await adapter.drain_outbox(agent, timeout=0.5)
    for m in msgs:
        print(f"[smoke] outbox -> {m.text!r}")

    await scheduler.stop()
    await loader.unload_from(agent)
    writer.detach()
    print(f"[smoke] phase={agent.phase}; trace at {trace_path}")
    assert agent.phase == LifecyclePhase.STOP


if __name__ == "__main__":
    asyncio.run(main())
