from __future__ import annotations

from mutsukibot.contracts import AgentId, Scopes
from mutsukibot.core.agent import Agent
from mutsukibot.core.loader import PluginLoader
from mutsukibot.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukibot.plugins.todo import TodoPlugin
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock
from tests.support.dispatcher_contract import assert_dispatcher_clean_after_unload


def _agent() -> Agent:
    return Agent(
        agent_id=AgentId("dispatcher-contract-fixture"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


async def test_contract_fixture_asserts_inmemory_and_todo_unload_clean() -> None:
    agent = _agent()
    loader = PluginLoader(allow={InMemoryEndpointPlugin.id, TodoPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, TodoPlugin])

    await assert_dispatcher_clean_after_unload(
        loader,
        agent,
        operations=(
            "todo:default.create",
            "todo:default.list",
            "todo:default.complete",
        ),
        sources=("inmemory:default",),
    )

