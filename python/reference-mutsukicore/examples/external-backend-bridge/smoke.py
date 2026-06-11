from __future__ import annotations

import asyncio
from pathlib import Path
import sys
from typing import ClassVar

import msgspec

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from mutsukicore import Capability, Caps, Plugin
from mutsukicore.contracts import (
    AgentId,
    BySchemaPrefix,
    BySourceKind,
    Envelope,
    EnvelopeId,
    OperationDescriptor,
    Perms,
    SourceDescriptor,
    SourceKindName,
    SourceRef,
)
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader
from mutsukicore.runtime import NanoIdGen, SeededRng, SystemClock

BackendKind = SourceKindName.register("example.todo_backend", declared_by="examples")


class TodoBackendSourceRef(SourceRef):
    schema_id: ClassVar[str] = "example.todo_backend.source_ref"
    schema_version: ClassVar[str] = "1.0.0"

    stream_id: str


class TodoBackendEvent(Envelope):
    schema_id: ClassVar[str] = "example.todo_backend.event"
    schema_version: ClassVar[str] = "1.0.0"

    event_type: str = ""
    payload: dict[str, str] = msgspec.field(default_factory=dict)


_SOURCE = SourceDescriptor(
    source_id="todo-backend:default",
    kind=BackendKind,
    capabilities=(),
    description="External todo backend event stream.",
)

_OP_NOTIFY = OperationDescriptor(
    op_id="todo-backend:default.notify",
    name="notify",
    description="Send an action to the external todo backend.",
    plugin_id="example-external-backend-bridge",
    requires_capabilities=(Caps.NETWORK_EGRESS,),
    parameters_schema={
        "type": "object",
        "properties": {"message": {"type": "string"}},
        "required": ["message"],
    },
    return_schema={"type": "string"},
)


class _Config(msgspec.Struct, kw_only=True):
    stream_id: str = "todo"


class ExternalTodoBackendBridge(Plugin[_Config]):
    id: ClassVar[str] = "example-external-backend-bridge"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.NETWORK_EGRESS)]
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = (_SOURCE,)
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (_OP_NOTIFY,)
    Config = _Config

    async def on_load(self) -> None:
        self.sent_actions: list[str] = []
        self.agent.dispatch.register_source(
            _SOURCE,
            plugin_scope=self.scope,
            plugin_id=self.id,
        )
        self.agent.dispatch.register_operation(
            _OP_NOTIFY,
            handler=self._notify_backend,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )

    async def publish_backend_event(
        self,
        event_type: str,
        payload: dict[str, str],
    ) -> TodoBackendEvent:
        event = TodoBackendEvent(
            id=EnvelopeId(self.agent.id_gen.next("backend-event")),
            timestamp=self.agent.clock.now(),
            source=TodoBackendSourceRef(
                source_id=_SOURCE.source_id,
                kind=BackendKind,
                stream_id=self.config.stream_id,
            ),
            payload_schema_id=f"example.todo_backend.{event_type}",
            event_type=event_type,
            payload=payload,
        )
        await self.agent.dispatch.publish(event)
        return event

    async def _notify_backend(self, _ctx, payload: dict[str, object]) -> str:
        message = str(payload.get("message", ""))
        self.sent_actions.append(message)
        return f"external-backend-accepted:{message}"


async def main() -> None:
    agent = Agent(
        agent_id=AgentId("external-backend-example"),
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(seed=0),
        accepts=(BySchemaPrefix("example.todo_backend.") & BySourceKind(BackendKind),),
    )
    loader = PluginLoader(allow={ExternalTodoBackendBridge.id})
    await loader.load_into(agent, [ExternalTodoBackendBridge])
    bridge = next(
        loaded.plugin
        for loaded in agent.plugins
        if isinstance(loaded.plugin, ExternalTodoBackendBridge)
    )

    event = await bridge.publish_backend_event("item_changed", {"id": "item-1"})
    received = await agent.inbox.get()
    assert received is event

    result = await agent.dispatch.invoke(
        "todo-backend:default.notify",
        {"message": "agent observed item-1"},
        ctx=agent.make_context(),
    )
    assert result == "external-backend-accepted:agent observed item-1"

    await loader.unload_from(agent)


if __name__ == "__main__":
    asyncio.run(main())
