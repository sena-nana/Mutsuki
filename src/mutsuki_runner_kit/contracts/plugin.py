from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    ScalarValue,
    as_bool,
    as_int,
    as_json_value,
    as_mapping,
    as_scalar_dict,
    as_str,
    as_str_dict,
    as_str_tuple,
    field_value,
    tuple_from_json,
)
from mutsuki_runner_kit.contracts.extension import (
    BridgeDescriptor,
    CodecDescriptor,
    HostExtensionDescriptor,
    PluginBackendDescriptor,
    PluginDeploymentKind,
    SchedulerPolicyDescriptor,
    WorkflowDescriptor,
)
from mutsuki_runner_kit.contracts.resource import ResourceTypeDescriptor
from mutsuki_runner_kit.contracts.runner import RunnerDescriptor
from mutsuki_runner_kit.contracts.surface import ContractSurface


class ArtifactType(StrEnum):
    ABI = "abi"
    PROCESS = "process"
    WASM = "wasm"
    PYTHON = "python"
    NATIVE = "native"


class RuntimeProfileMode(StrEnum):
    FULL_DEV = "full_dev"
    EXTENSIBLE_RUNTIME = "extensible_runtime"
    BUILTIN_ONLY = "builtin_only"
    LOCKED_BUILTIN = "locked_builtin"


def as_plugin_deployments(value: object, field: str) -> dict[str, PluginDeploymentKind]:
    raw = as_mapping(value, field)
    return {
        str(plugin_id): PluginDeploymentKind(as_str(deployment, field))
        for plugin_id, deployment in raw.items()
    }


@dataclass(frozen=True)
class PluginArtifact:
    artifact_type: ArtifactType
    path: str
    sha256: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "PluginArtifact")
        return cls(
            artifact_type=ArtifactType(as_str(field_value(raw, "artifact_type"), "artifact_type")),
            path=as_str(field_value(raw, "path"), "path"),
            sha256=as_str(field_value(raw, "sha256"), "sha256"),
        )


@dataclass(frozen=True)
class PermissionGrant:
    effects: tuple[str, ...]
    resources: tuple[str, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "PermissionGrant")
        return cls(
            effects=as_str_tuple(field_value(raw, "effects"), "effects"),
            resources=as_str_tuple(field_value(raw, "resources"), "resources"),
        )


@dataclass(frozen=True)
class LifecyclePolicy:
    reload_policy: str
    unload_timeout_ms: int
    supports_cancel: bool
    supports_dispose: bool
    supports_snapshot: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "LifecyclePolicy")
        return cls(
            reload_policy=as_str(field_value(raw, "reload_policy"), "reload_policy"),
            unload_timeout_ms=as_int(field_value(raw, "unload_timeout_ms"), "unload_timeout_ms"),
            supports_cancel=as_bool(field_value(raw, "supports_cancel"), "supports_cancel"),
            supports_dispose=as_bool(field_value(raw, "supports_dispose"), "supports_dispose"),
            supports_snapshot=as_bool(field_value(raw, "supports_snapshot"), "supports_snapshot"),
        )


@dataclass(frozen=True)
class ProtocolDescriptor:
    protocol_id: str
    version: str
    input_schema: JsonValue
    output_schema: JsonValue
    error_schema: JsonValue
    codec: str
    compatibility: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ProtocolDescriptor")
        return cls(
            protocol_id=as_str(field_value(raw, "protocol_id"), "protocol_id"),
            version=as_str(field_value(raw, "version"), "version"),
            input_schema=as_json_value(field_value(raw, "input_schema")),
            output_schema=as_json_value(field_value(raw, "output_schema")),
            error_schema=as_json_value(field_value(raw, "error_schema")),
            codec=as_str(field_value(raw, "codec"), "codec"),
            compatibility=as_str(field_value(raw, "compatibility"), "compatibility"),
        )


@dataclass(frozen=True)
class HandlerBinding:
    binding_id: str
    plugin_id: str
    protocol_id: str
    target_protocol_id: str
    target_runner_hint: str | None
    pool_id: str
    priority: int
    policy: str
    metadata: dict[str, ScalarValue]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "HandlerBinding")
        target_runner_hint = field_value(raw, "target_runner_hint")
        return cls(
            binding_id=as_str(field_value(raw, "binding_id"), "binding_id"),
            plugin_id=as_str(field_value(raw, "plugin_id"), "plugin_id"),
            protocol_id=as_str(field_value(raw, "protocol_id"), "protocol_id"),
            target_protocol_id=as_str(field_value(raw, "target_protocol_id"), "target_protocol_id"),
            target_runner_hint=None
            if target_runner_hint is None
            else as_str(target_runner_hint, "target_runner_hint"),
            pool_id=as_str(field_value(raw, "pool_id"), "pool_id"),
            priority=as_int(field_value(raw, "priority"), "priority"),
            policy=as_str(field_value(raw, "policy"), "policy"),
            metadata=as_scalar_dict(field_value(raw, "metadata"), "metadata"),
        )


@dataclass(frozen=True)
class PluginProvides:
    runners: tuple[RunnerDescriptor, ...]
    protocols: tuple[ProtocolDescriptor, ...]
    handler_bindings: tuple[HandlerBinding, ...]
    resource_schemas: tuple[str, ...]
    resource_providers: tuple[str, ...]
    resource_types: tuple[ResourceTypeDescriptor, ...]
    effects: tuple[str, ...]
    streams: tuple[str, ...]
    subscriptions: tuple[str, ...]
    timers: tuple[str, ...]
    state_schemas: tuple[str, ...]
    host_extensions: tuple[HostExtensionDescriptor, ...]
    plugin_backends: tuple[PluginBackendDescriptor, ...]
    codecs: tuple[CodecDescriptor, ...]
    bridges: tuple[BridgeDescriptor, ...]
    scheduler_policies: tuple[SchedulerPolicyDescriptor, ...]
    workflows: tuple[WorkflowDescriptor, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "PluginProvides")
        return cls(
            runners=tuple_from_json(raw, "runners", RunnerDescriptor),
            protocols=tuple_from_json(raw, "protocols", ProtocolDescriptor),
            handler_bindings=tuple_from_json(raw, "handler_bindings", HandlerBinding),
            resource_schemas=as_str_tuple(field_value(raw, "resource_schemas"), "resource_schemas"),
            resource_providers=as_str_tuple(
                field_value(raw, "resource_providers"), "resource_providers"
            ),
            resource_types=tuple_from_json(raw, "resource_types", ResourceTypeDescriptor),
            effects=as_str_tuple(field_value(raw, "effects"), "effects"),
            streams=as_str_tuple(field_value(raw, "streams"), "streams"),
            subscriptions=as_str_tuple(field_value(raw, "subscriptions"), "subscriptions"),
            timers=as_str_tuple(field_value(raw, "timers"), "timers"),
            state_schemas=as_str_tuple(field_value(raw, "state_schemas"), "state_schemas"),
            host_extensions=tuple_from_json(raw, "host_extensions", HostExtensionDescriptor),
            plugin_backends=tuple_from_json(raw, "plugin_backends", PluginBackendDescriptor),
            codecs=tuple_from_json(raw, "codecs", CodecDescriptor),
            bridges=tuple_from_json(raw, "bridges", BridgeDescriptor),
            scheduler_policies=tuple_from_json(
                raw, "scheduler_policies", SchedulerPolicyDescriptor
            ),
            workflows=tuple_from_json(raw, "workflows", WorkflowDescriptor),
        )


@dataclass(frozen=True)
class PluginManifest:
    plugin_id: str
    version: str
    api_version: str
    artifact: PluginArtifact
    provides: PluginProvides
    requires: tuple[str, ...]
    permissions: PermissionGrant
    lifecycle: LifecyclePolicy
    metadata: dict[str, ScalarValue]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "PluginManifest")
        return cls(
            plugin_id=as_str(field_value(raw, "plugin_id"), "plugin_id"),
            version=as_str(field_value(raw, "version"), "version"),
            api_version=as_str(field_value(raw, "api_version"), "api_version"),
            artifact=PluginArtifact.from_json_dict(
                as_mapping(field_value(raw, "artifact"), "artifact")
            ),
            provides=PluginProvides.from_json_dict(
                as_mapping(field_value(raw, "provides"), "provides")
            ),
            requires=as_str_tuple(field_value(raw, "requires"), "requires"),
            permissions=PermissionGrant.from_json_dict(
                as_mapping(field_value(raw, "permissions"), "permissions")
            ),
            lifecycle=LifecyclePolicy.from_json_dict(
                as_mapping(field_value(raw, "lifecycle"), "lifecycle")
            ),
            metadata=as_scalar_dict(field_value(raw, "metadata"), "metadata"),
        )


@dataclass(frozen=True)
class RuntimeProfile:
    profile_id: str
    mode: RuntimeProfileMode
    enabled_plugins: tuple[str, ...]
    bindings: dict[str, str]
    plugin_deployments: dict[str, PluginDeploymentKind]
    allow_dynamic_registration: bool
    allow_hot_reload: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RuntimeProfile")
        return cls(
            profile_id=as_str(field_value(raw, "profile_id"), "profile_id"),
            mode=RuntimeProfileMode(as_str(field_value(raw, "mode"), "mode")),
            enabled_plugins=as_str_tuple(field_value(raw, "enabled_plugins"), "enabled_plugins"),
            bindings=as_str_dict(raw, "bindings"),
            plugin_deployments=as_plugin_deployments(
                field_value(raw, "plugin_deployments"), "plugin_deployments"
            ),
            allow_dynamic_registration=as_bool(
                field_value(raw, "allow_dynamic_registration"), "allow_dynamic_registration"
            ),
            allow_hot_reload=as_bool(field_value(raw, "allow_hot_reload"), "allow_hot_reload"),
        )


@dataclass(frozen=True)
class CapabilityProviderSelection:
    capability: str
    provider_plugin_id: str
    provider_version: str | None
    surface_id: str
    reason: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "CapabilityProviderSelection")
        provider_version = field_value(raw, "provider_version")
        return cls(
            capability=as_str(field_value(raw, "capability"), "capability"),
            provider_plugin_id=as_str(field_value(raw, "provider_plugin_id"), "provider_plugin_id"),
            provider_version=None
            if provider_version is None
            else as_str(provider_version, "provider_version"),
            surface_id=as_str(field_value(raw, "surface_id"), "surface_id"),
            reason=as_str(field_value(raw, "reason"), "reason"),
        )


@dataclass(frozen=True)
class PermissionAuditEntry:
    plugin_id: str
    permission_kind: str
    permission: str
    granted: bool
    provider_capability: str | None
    reason: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "PermissionAuditEntry")
        provider_capability = field_value(raw, "provider_capability")
        return cls(
            plugin_id=as_str(field_value(raw, "plugin_id"), "plugin_id"),
            permission_kind=as_str(field_value(raw, "permission_kind"), "permission_kind"),
            permission=as_str(field_value(raw, "permission"), "permission"),
            granted=as_bool(field_value(raw, "granted"), "granted"),
            provider_capability=None
            if provider_capability is None
            else as_str(provider_capability, "provider_capability"),
            reason=as_str(field_value(raw, "reason"), "reason"),
        )


@dataclass(frozen=True)
class RuntimeCapabilityGraph:
    profile_mode: RuntimeProfileMode
    provided_capabilities: tuple[str, ...]
    required_capabilities: tuple[str, ...]
    active_capabilities: tuple[str, ...]
    active_capability_providers: tuple[CapabilityProviderSelection, ...]
    active_resource_providers: tuple[str, ...]
    active_host_extensions: tuple[str, ...]
    active_plugin_backends: tuple[str, ...]
    active_codecs: tuple[str, ...]
    active_bridges: tuple[str, ...]
    active_scheduler_policies: tuple[str, ...]
    active_workflows: tuple[str, ...]
    permission_audit: tuple[PermissionAuditEntry, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RuntimeCapabilityGraph")
        return cls(
            profile_mode=RuntimeProfileMode(
                as_str(field_value(raw, "profile_mode"), "profile_mode")
            ),
            provided_capabilities=as_str_tuple(
                field_value(raw, "provided_capabilities"), "provided_capabilities"
            ),
            required_capabilities=as_str_tuple(
                field_value(raw, "required_capabilities"), "required_capabilities"
            ),
            active_capabilities=as_str_tuple(
                field_value(raw, "active_capabilities"), "active_capabilities"
            ),
            active_capability_providers=tuple_from_json(
                raw, "active_capability_providers", CapabilityProviderSelection
            ),
            active_resource_providers=as_str_tuple(
                field_value(raw, "active_resource_providers"),
                "active_resource_providers",
            ),
            active_host_extensions=as_str_tuple(
                field_value(raw, "active_host_extensions"), "active_host_extensions"
            ),
            active_plugin_backends=as_str_tuple(
                field_value(raw, "active_plugin_backends"), "active_plugin_backends"
            ),
            active_codecs=as_str_tuple(field_value(raw, "active_codecs"), "active_codecs"),
            active_bridges=as_str_tuple(field_value(raw, "active_bridges"), "active_bridges"),
            active_scheduler_policies=as_str_tuple(
                field_value(raw, "active_scheduler_policies"),
                "active_scheduler_policies",
            ),
            active_workflows=as_str_tuple(field_value(raw, "active_workflows"), "active_workflows"),
            permission_audit=tuple_from_json(raw, "permission_audit", PermissionAuditEntry),
        )


@dataclass(frozen=True)
class RuntimeLoadPlan:
    lock_version: int
    core_api_version: str
    profile_id: str
    profile_hash: str
    registry_generation: int
    plugins: tuple[PluginManifest, ...]
    load_order: tuple[str, ...]
    runner_bindings: dict[str, str]
    plugin_deployments: dict[str, PluginDeploymentKind]
    capability_graph: RuntimeCapabilityGraph
    contract_surfaces: tuple[ContractSurface, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RuntimeLoadPlan")
        return cls(
            lock_version=as_int(field_value(raw, "lock_version"), "lock_version"),
            core_api_version=as_str(field_value(raw, "core_api_version"), "core_api_version"),
            profile_id=as_str(field_value(raw, "profile_id"), "profile_id"),
            profile_hash=as_str(field_value(raw, "profile_hash"), "profile_hash"),
            registry_generation=as_int(
                field_value(raw, "registry_generation"), "registry_generation"
            ),
            plugins=tuple_from_json(raw, "plugins", PluginManifest),
            load_order=as_str_tuple(field_value(raw, "load_order"), "load_order"),
            runner_bindings=as_str_dict(raw, "runner_bindings"),
            plugin_deployments=as_plugin_deployments(
                field_value(raw, "plugin_deployments"), "plugin_deployments"
            ),
            capability_graph=RuntimeCapabilityGraph.from_json_dict(
                as_mapping(field_value(raw, "capability_graph"), "capability_graph")
            ),
            contract_surfaces=tuple_from_json(raw, "contract_surfaces", ContractSurface),
        )


RuntimeLock = RuntimeLoadPlan
