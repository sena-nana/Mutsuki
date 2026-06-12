from __future__ import annotations

from typing import ClassVar

import msgspec

from mutsuki import Capability, Caps, Plugin
from mutsuki.contracts import (
    Envelope,
    EnvelopeId,
    OperationDescriptor,
    Perms,
    SourceDescriptor,
    SourceKindName,
    SourceRef,
)

BackendKind = SourceKindName.register("example.backend", declared_by="tests")


class BackendSourceRef(SourceRef):
    schema_id: ClassVar[str] = "tests.external_backend.source_ref"
    schema_version: ClassVar[str] = "1.0.0"

    stream_id: str


class BackendEvent(Envelope):
    schema_id: ClassVar[str] = "tests.external_backend.event"
    schema_version: ClassVar[str] = "1.0.0"

    event_type: str = ""
    payload: dict[str, str] = msgspec.field(default_factory=dict)


_SOURCE = SourceDescriptor(
    source_id="backend:default",
    kind=BackendKind,
    capabilities=(),
    description="Test-only external backend event source.",
)

_OP_NOTIFY = OperationDescriptor(
    op_id="backend:default.notify",
    name="notify",
    description="Simulate an action against an external backend.",
    plugin_id="tests-external-backend-bridge",
    requires_capabilities=(Caps.NETWORK_EGRESS,),
    parameters_schema={
        "type": "object",
        "properties": {"message": {"type": "string"}},
        "required": ["message"],
    },
    return_schema={"type": "string"},
)


class _Config(msgspec.Struct, kw_only=True):
    stream_id: str = "todo-events"


class ExternalBackendBridgePlugin(Plugin[_Config]):
    id: ClassVar[str] = "tests-external-backend-bridge"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.NETWORK_EGRESS),
    ]
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = (_SOURCE,)
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (_OP_NOTIFY,)
    Config = _Config

    async def on_load(self) -> None:
        self.notifications: list[str] = []
        self.agent.dispatch.register_source(
            _SOURCE,
            plugin_scope=self.scope,
            plugin_id=self.id,
        )
        self.agent.dispatch.register_operation(
            _OP_NOTIFY,
            handler=self._notify,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )

    async def publish_event(self, event_type: str, payload: dict[str, str]) -> BackendEvent:
        event = BackendEvent(
            id=EnvelopeId(self.agent.id_gen.next("backend-event")),
            timestamp=self.agent.clock.now(),
            source=BackendSourceRef(
                source_id=_SOURCE.source_id,
                kind=BackendKind,
                stream_id=self.config.stream_id,
            ),
            payload_schema_id=f"example.backend.{event_type}",
            event_type=event_type,
            payload=payload,
        )
        await self.agent.dispatch.publish(event)
        return event

    async def _notify(self, _ctx, payload: dict[str, object]) -> str:
        message = str(payload.get("message", ""))
        self.notifications.append(message)
        return f"sent:{message}"


__all__ = [
    "BackendEvent",
    "BackendKind",
    "BackendSourceRef",
    "ExternalBackendBridgePlugin",
]
