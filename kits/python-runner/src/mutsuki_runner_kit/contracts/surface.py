from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    as_bool,
    as_int,
    as_mapping,
    as_str,
    field_value,
)


class ContractSurfaceKind(StrEnum):
    RUNNER = "runner"
    TASK_PROTOCOL = "task_protocol"
    SCHEMA = "schema"
    RESOURCE_SCHEMA = "resource_schema"
    RESOURCE_PROVIDER = "resource_provider"
    HOST_EXTENSION = "host_extension"
    PLUGIN_BACKEND = "plugin_backend"
    CODEC = "codec"
    BRIDGE = "bridge"
    SCHEDULER_POLICY = "scheduler_policy"
    WORKFLOW = "workflow"
    EFFECT = "effect"
    STREAM = "stream"
    SUBSCRIPTION = "subscription"
    TIMER = "timer"
    PROTOCOL = "protocol"
    HANDLER_BINDING = "handler_binding"
    STATE_SCHEMA = "state_schema"
    LIFECYCLE = "lifecycle"
    PERMISSION = "permission"


class SurfaceCompatibility(StrEnum):
    IDENTICAL = "identical"
    ADDITIVE = "additive"
    DEPRECATED = "deprecated"
    REMOVED = "removed"
    BREAKING = "breaking"


class SurfaceOccupancyHandleKind(StrEnum):
    STREAM = "stream"
    SUBSCRIPTION = "subscription"
    TIMER = "timer"


@dataclass(frozen=True)
class ContractSurface:
    surface_id: str
    kind: ContractSurfaceKind
    owner_plugin_id: str
    fingerprint: str
    deprecated: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ContractSurface")
        return cls(
            surface_id=as_str(field_value(raw, "surface_id"), "surface_id"),
            kind=ContractSurfaceKind(as_str(field_value(raw, "kind"), "kind")),
            owner_plugin_id=as_str(field_value(raw, "owner_plugin_id"), "owner_plugin_id"),
            fingerprint=as_str(field_value(raw, "fingerprint"), "fingerprint"),
            deprecated=as_bool(field_value(raw, "deprecated"), "deprecated"),
        )


@dataclass(frozen=True)
class SurfaceOccupancy:
    surface_id: str
    ready_tasks: int
    running_invocations: int
    resource_refs: int
    state_refs: int
    active_leases: int
    open_streams: int
    subscriptions: int
    timers: int
    effect_inflight: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "SurfaceOccupancy")
        return cls(
            surface_id=as_str(field_value(raw, "surface_id"), "surface_id"),
            ready_tasks=as_int(field_value(raw, "ready_tasks"), "ready_tasks"),
            running_invocations=as_int(
                field_value(raw, "running_invocations"), "running_invocations"
            ),
            resource_refs=as_int(field_value(raw, "resource_refs"), "resource_refs"),
            state_refs=as_int(field_value(raw, "state_refs"), "state_refs"),
            active_leases=as_int(field_value(raw, "active_leases"), "active_leases"),
            open_streams=as_int(field_value(raw, "open_streams"), "open_streams"),
            subscriptions=as_int(field_value(raw, "subscriptions"), "subscriptions"),
            timers=as_int(field_value(raw, "timers"), "timers"),
            effect_inflight=as_int(field_value(raw, "effect_inflight"), "effect_inflight"),
        )

    def is_zero(self) -> bool:
        return (
            self.ready_tasks == 0
            and self.running_invocations == 0
            and self.resource_refs == 0
            and self.state_refs == 0
            and self.active_leases == 0
            and self.open_streams == 0
            and self.subscriptions == 0
            and self.timers == 0
            and self.effect_inflight == 0
        )


@dataclass(frozen=True)
class SurfaceOccupancyHandle:
    handle_id: str
    surface_id: str
    owner_plugin_id: str
    plugin_generation: int
    registry_generation: int
    kind: SurfaceOccupancyHandleKind

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "SurfaceOccupancyHandle")
        return cls(
            handle_id=as_str(field_value(raw, "handle_id"), "handle_id"),
            surface_id=as_str(field_value(raw, "surface_id"), "surface_id"),
            owner_plugin_id=as_str(field_value(raw, "owner_plugin_id"), "owner_plugin_id"),
            plugin_generation=as_int(field_value(raw, "plugin_generation"), "plugin_generation"),
            registry_generation=as_int(
                field_value(raw, "registry_generation"), "registry_generation"
            ),
            kind=SurfaceOccupancyHandleKind(as_str(field_value(raw, "kind"), "kind")),
        )
