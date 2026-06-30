from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    as_bool,
    as_mapping,
    as_str,
    field_value,
    sequence,
)


class HostExtensionKind(StrEnum):
    PLUGIN_BACKEND = "plugin_backend"
    BRIDGE = "bridge"
    CODEC = "codec"
    TRACE_SINK = "trace_sink"
    SCHEDULER_POLICY = "scheduler_policy"
    PERMISSION_POLICY = "permission_policy"
    RESOURCE_PLAN_GATEWAY = "resource_plan_gateway"


class PluginDeploymentKind(StrEnum):
    BUILTIN = "builtin"
    ABI = "abi"
    WASM = "wasm"
    PROCESS = "process"
    PYTHON = "python"


@dataclass(frozen=True)
class HostExtensionDescriptor:
    extension_id: str
    kind: HostExtensionKind
    supported_deployments: tuple[PluginDeploymentKind, ...]
    reload_policy: str
    drain_required: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "HostExtensionDescriptor")
        return cls(
            extension_id=as_str(field_value(raw, "extension_id"), "extension_id"),
            kind=HostExtensionKind(as_str(field_value(raw, "kind"), "kind")),
            supported_deployments=tuple(
                PluginDeploymentKind(as_str(item, "supported_deployments"))
                for item in sequence(
                    field_value(raw, "supported_deployments"), "supported_deployments"
                )
            ),
            reload_policy=as_str(field_value(raw, "reload_policy"), "reload_policy"),
            drain_required=as_bool(field_value(raw, "drain_required"), "drain_required"),
        )


@dataclass(frozen=True)
class PluginBackendDescriptor:
    backend_id: str
    deployment_kind: PluginDeploymentKind
    task_client_protocol: str
    resource_client_protocol: str
    codec_id: str | None
    bridge_id: str | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "PluginBackendDescriptor")
        codec_id = field_value(raw, "codec_id")
        bridge_id = field_value(raw, "bridge_id")
        return cls(
            backend_id=as_str(field_value(raw, "backend_id"), "backend_id"),
            deployment_kind=PluginDeploymentKind(
                as_str(field_value(raw, "deployment_kind"), "deployment_kind")
            ),
            task_client_protocol=as_str(
                field_value(raw, "task_client_protocol"), "task_client_protocol"
            ),
            resource_client_protocol=as_str(
                field_value(raw, "resource_client_protocol"), "resource_client_protocol"
            ),
            codec_id=None if codec_id is None else as_str(codec_id, "codec_id"),
            bridge_id=None if bridge_id is None else as_str(bridge_id, "bridge_id"),
        )


@dataclass(frozen=True)
class CodecDescriptor:
    codec_id: str
    media_type: str
    version: str
    connection_scoped: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "CodecDescriptor")
        return cls(
            codec_id=as_str(field_value(raw, "codec_id"), "codec_id"),
            media_type=as_str(field_value(raw, "media_type"), "media_type"),
            version=as_str(field_value(raw, "version"), "version"),
            connection_scoped=as_bool(
                field_value(raw, "connection_scoped"), "connection_scoped"
            ),
        )


@dataclass(frozen=True)
class BridgeDescriptor:
    bridge_id: str
    deployment_kind: PluginDeploymentKind
    codec_ids: tuple[str, ...]
    drain_policy: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "BridgeDescriptor")
        return cls(
            bridge_id=as_str(field_value(raw, "bridge_id"), "bridge_id"),
            deployment_kind=PluginDeploymentKind(
                as_str(field_value(raw, "deployment_kind"), "deployment_kind")
            ),
            codec_ids=tuple(
                as_str(item, "codec_ids")
                for item in sequence(field_value(raw, "codec_ids"), "codec_ids")
            ),
            drain_policy=as_str(field_value(raw, "drain_policy"), "drain_policy"),
        )


@dataclass(frozen=True)
class SchedulerPolicyDescriptor:
    policy_id: str
    version: str
    decision_scope: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "SchedulerPolicyDescriptor")
        return cls(
            policy_id=as_str(field_value(raw, "policy_id"), "policy_id"),
            version=as_str(field_value(raw, "version"), "version"),
            decision_scope=as_str(field_value(raw, "decision_scope"), "decision_scope"),
        )


@dataclass(frozen=True)
class WorkflowDescriptor:
    workflow_id: str
    state_resource_kind: str
    runner_protocol_id: str
    reload_policy: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "WorkflowDescriptor")
        return cls(
            workflow_id=as_str(field_value(raw, "workflow_id"), "workflow_id"),
            state_resource_kind=as_str(
                field_value(raw, "state_resource_kind"), "state_resource_kind"
            ),
            runner_protocol_id=as_str(
                field_value(raw, "runner_protocol_id"), "runner_protocol_id"
            ),
            reload_policy=as_str(field_value(raw, "reload_policy"), "reload_policy"),
        )
