from __future__ import annotations

from mutsukicore.contracts import AgentId, BySchemaPrefix, BySourceKind, Scopes
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader
from mutsukicore.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukicore.runtime import DeterministicIdGen, SeededRng, SystemClock
from tests.support.dispatcher_contract import assert_dispatcher_clean_after_unload
from tests.support.external_backend_bridge import (
    BackendEvent,
    BackendKind,
    ExternalBackendBridgePlugin,
)


def _agent() -> Agent:
    return Agent(
        agent_id=AgentId("dispatcher-contract-fixture"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )


async def test_contract_fixture_asserts_inmemory_and_backend_bridge_unload_clean() -> None:
    agent = _agent()
    loader = PluginLoader(
        allow={InMemoryEndpointPlugin.id, ExternalBackendBridgePlugin.id}
    )
    await loader.load_into(agent, [InMemoryEndpointPlugin, ExternalBackendBridgePlugin])

    await assert_dispatcher_clean_after_unload(
        loader,
        agent,
        operations=("backend:default.notify",),
        sources=("inmemory:default", "backend:default"),
    )


async def test_external_backend_bridge_routes_custom_event_and_invokes_action() -> None:
    agent = Agent(
        agent_id=AgentId("backend-bridge-fixture"),
        clock=SystemClock(),
        id_gen=DeterministicIdGen(),
        rng=SeededRng(seed=0),
        accepts=(
            Scopes.IM_TEXT.to_rule(),
            BySchemaPrefix("example.backend.") & BySourceKind(BackendKind),
        ),
    )
    loader = PluginLoader(allow={ExternalBackendBridgePlugin.id})
    await loader.load_into(agent, [ExternalBackendBridgePlugin])

    bridge = next(
        loaded.plugin
        for loaded in agent.plugins
        if isinstance(loaded.plugin, ExternalBackendBridgePlugin)
    )
    event = await bridge.publish_event("item_changed", {"id": "item-1"})
    received = await agent.inbox.get()
    assert received is event
    assert isinstance(received, BackendEvent)
    assert received.payload == {"id": "item-1"}

    result = await agent.dispatch.invoke(
        "backend:default.notify",
        {"message": "agent saw item-1"},
        ctx=agent.make_context(),
    )
    assert result == "sent:agent saw item-1"
    assert bridge.notifications == ["agent saw item-1"]

    await assert_dispatcher_clean_after_unload(
        loader,
        agent,
        operations=("backend:default.notify",),
        sources=("backend:default",),
    )
