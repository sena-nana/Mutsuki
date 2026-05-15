"""跨 endpoint 协作冒烟脚本 —— v0.2 Phase B 通过门。

验证：一条 IM 消息（``todo 买菜``）通过 ``qq_to_todo`` plugin 的 envelope
二次分发触发 ``ctx.dispatch.invoke("todo:default.create", ...)``，写入 todo
endpoint。

用法::

    python -m mutsukibot.plugins.qq_to_todo.smoke
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from tempfile import gettempdir

from mutsukibot.contracts import Scopes
from mutsukibot.contracts.ids import AgentId
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.core.agent import Agent
from mutsukibot.core.loader import PluginLoader
from mutsukibot.observability import JsonlTraceWriter
from mutsukibot.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukibot.plugins.qq_to_todo import QqToTodoPlugin
from mutsukibot.plugins.todo import TodoPlugin
from mutsukibot.runtime import NanoIdGen, SeededRng, SystemClock
from mutsukibot.runtime.scheduler import AgentScheduler


async def main() -> None:
    agent = Agent(
        agent_id=AgentId("bridge-smoke"),
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )
    trace_path = Path(gettempdir()) / "mutsukibot-qq-to-todo-smoke.jsonl"
    writer = JsonlTraceWriter(trace_path)
    writer.attach(agent.bus)

    # 装载顺序由 PluginLoader DAG 自动决定：QqToTodoPlugin 依赖 todo:default.create，
    # 故 TodoPlugin 必先装载（D9b 拓扑）。
    loader = PluginLoader(
        allow={
            InMemoryEndpointPlugin.id,
            TodoPlugin.id,
            QqToTodoPlugin.id,
        }
    )
    await loader.load_into(
        agent,
        [InMemoryEndpointPlugin, TodoPlugin, QqToTodoPlugin],
    )
    print(f"[smoke] agent {agent.agent_id} phase={agent.phase}")
    print(f"[smoke] loaded plugins (DAG order): {[p.plugin.id for p in agent.plugins]}")
    print(f"[smoke] dispatcher ops: {[op.op_id for op in agent.dispatch.list_operations()]}")

    scheduler = AgentScheduler(agent)
    await scheduler.start()

    inmem = next(
        p.plugin for p in agent.plugins if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    # 发一条 IM 消息：qq_to_todo 在 on_envelope 中匹配前缀 → 调 todo create
    await inmem.send_text("todo 买菜")
    await asyncio.sleep(0.3)
    # 也发一条不匹配前缀的，验证不会写入
    await inmem.send_text("hello world")
    await asyncio.sleep(0.2)

    # 直接通过 dispatcher 查 todo list 验证写入
    ctx = agent.make_context()
    items = await agent.dispatch.invoke("todo:default.list", {}, ctx=ctx)
    print(f"[smoke] todo items after bridge: {items}")
    assert len(items) == 1, f"期望 1 个 todo item，实际 {len(items)}"
    assert items[0]["text"] == "买菜"

    await scheduler.stop()
    await loader.unload_from(agent)
    writer.detach()
    print(f"[smoke] phase={agent.phase}; trace at {trace_path}")
    assert agent.phase == LifecyclePhase.STOP


if __name__ == "__main__":
    asyncio.run(main())
