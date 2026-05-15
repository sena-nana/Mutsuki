"""Todo plugin 冒烟脚本 —— v0.2 工具型 endpoint 单插件验证。

通过 ``ctx.dispatch.invoke`` 直接调用 todo 三个 Operation，验证 Hard
rule #14 演示路径（Handle 持有 in-memory dict + scope 关闭后 Handle 释放）。

用法::

    python -m mutsukibot.plugins.todo.smoke
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from tempfile import gettempdir

from mutsukibot.contracts.ids import AgentId
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.core.agent import Agent
from mutsukibot.core.loader import PluginLoader
from mutsukibot.observability import JsonlTraceWriter
from mutsukibot.plugins.todo import TodoPlugin
from mutsukibot.runtime import NanoIdGen, SeededRng, SystemClock


async def main() -> None:
    agent = Agent(
        agent_id=AgentId("todo-smoke"),
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(seed=0),
        accepts=(),  # 工具型 endpoint 测试不接收 envelope，仅 invoke
    )
    trace_path = Path(gettempdir()) / "mutsukibot-todo-smoke.jsonl"
    writer = JsonlTraceWriter(trace_path)
    writer.attach(agent.bus)

    loader = PluginLoader(allow={TodoPlugin.id})
    await loader.load_into(agent, [TodoPlugin])

    print(f"[smoke] agent {agent.agent_id} phase={agent.phase}")
    print(f"[smoke] dispatcher ops: {[op.op_id for op in agent.dispatch.list_operations()]}")

    ctx = agent.make_context()

    item_a = await agent.dispatch.invoke(
        "todo:default.create", {"text": "写报告"}, ctx=ctx
    )
    item_b = await agent.dispatch.invoke(
        "todo:default.create", {"text": "买菜"}, ctx=ctx
    )
    print(f"[smoke] created: {item_a}, {item_b}")

    items = await agent.dispatch.invoke("todo:default.list", {}, ctx=ctx)
    print(f"[smoke] list (initial): {items}")
    assert len(items) == 2

    ok = await agent.dispatch.invoke(
        "todo:default.complete", {"item_id": item_a}, ctx=ctx
    )
    print(f"[smoke] complete {item_a} -> {ok}")

    items = await agent.dispatch.invoke("todo:default.list", {}, ctx=ctx)
    print(f"[smoke] list (after complete): {items}")
    done_ids = {i["id"] for i in items if i["done"]}
    assert done_ids == {item_a}

    await loader.unload_from(agent)
    # 卸载后 dispatcher 已无 op 注册（PluginScope.close 触发 dispatcher 反注册）
    assert agent.dispatch.list_operations() == ()
    writer.detach()
    agent.phase = LifecyclePhase.STOP  # 没启 scheduler；显式标记
    print(f"[smoke] phase={agent.phase}; trace at {trace_path}")


if __name__ == "__main__":
    asyncio.run(main())
