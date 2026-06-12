"""跨 Agent 广播冒烟脚本 —— v0.2 Phase C 验收入口。

验证：一个 control Agent 通过 in-memory endpoint 发布 IM 消息时，
``Dispatcher.publish`` 经由 ``AgentRegistry`` 广播给所有 awake 且
``accepts`` 匹配的 Agent。control / audit 两个 Agent 都应收到同一条
envelope。

用法::

    python -m mutsuki.plugins.cross_agent_smoke
"""

from __future__ import annotations

import asyncio

from mutsuki.contracts import Scopes
from mutsuki.contracts.ids import AgentId
from mutsuki.contracts.message import Message
from mutsuki.core.agent import Agent
from mutsuki.core.agent_registry import AgentRegistry
from mutsuki.core.loader import PluginLoader
from mutsuki.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsuki.runtime import NanoIdGen, SeededRng, SystemClock


def _make_agent(agent_id: str) -> Agent:
    return Agent(
        agent_id=AgentId(agent_id),
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


async def main() -> None:
    AgentRegistry.clear()
    loader = PluginLoader(allow={InMemoryEndpointPlugin.id})
    control = _make_agent("cross-agent-control")
    audit = _make_agent("cross-agent-audit")

    try:
        await loader.load_into(control, [InMemoryEndpointPlugin])
        inmem = next(
            p.plugin
            for p in control.plugins
            if isinstance(p.plugin, InMemoryEndpointPlugin)
        )

        sent = await inmem.send_text("hello agents")
        control_msg = await asyncio.wait_for(control.inbox.get(), timeout=0.5)
        audit_msg = await asyncio.wait_for(audit.inbox.get(), timeout=0.5)

        assert isinstance(control_msg, Message)
        assert isinstance(audit_msg, Message)
        assert control_msg.id == sent.id
        assert audit_msg.id == sent.id
        assert control_msg.text == "hello agents"
        assert audit_msg.text == "hello agents"

        print(
            "[smoke] cross-agent broadcast -> "
            f"{control.agent_id}, {audit.agent_id}; message={sent.id}"
        )
    finally:
        await loader.unload_from(control)
        AgentRegistry.clear()


if __name__ == "__main__":
    asyncio.run(main())
