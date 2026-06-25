from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runtime_python.contracts.codec import (
    JsonDict,
    ScalarValue,
    as_bool,
    as_int,
    as_mapping,
    as_scalar_dict,
    as_str,
    as_str_dict,
    as_str_tuple,
    field_value,
    tuple_from_json,
)
from mutsuki_runtime_python.contracts.runner import RunnerDescriptor
from mutsuki_runtime_python.contracts.surface import ContractSurface
from mutsuki_runtime_python.contracts.task import TaskDemand


class ArtifactType(StrEnum):
    ABI = "abi"
    PROCESS = "process"
    WASM = "wasm"
    PYTHON = "python"
    NATIVE = "native"


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
class PluginProvides:
    runners: tuple[RunnerDescriptor, ...]
    task_demands: tuple[TaskDemand, ...]
    resource_schemas: tuple[str, ...]
    resource_providers: tuple[str, ...]
    effects: tuple[str, ...]
    streams: tuple[str, ...]
    subscriptions: tuple[str, ...]
    timers: tuple[str, ...]
    state_schemas: tuple[str, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "PluginProvides")
        return cls(
            runners=tuple_from_json(raw, "runners", RunnerDescriptor),
            task_demands=tuple_from_json(raw, "task_demands", TaskDemand),
            resource_schemas=as_str_tuple(field_value(raw, "resource_schemas"), "resource_schemas"),
            resource_providers=as_str_tuple(
                field_value(raw, "resource_providers"), "resource_providers"
            ),
            effects=as_str_tuple(field_value(raw, "effects"), "effects"),
            streams=as_str_tuple(field_value(raw, "streams"), "streams"),
            subscriptions=as_str_tuple(field_value(raw, "subscriptions"), "subscriptions"),
            timers=as_str_tuple(field_value(raw, "timers"), "timers"),
            state_schemas=as_str_tuple(field_value(raw, "state_schemas"), "state_schemas"),
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
    enabled_plugins: tuple[str, ...]
    bindings: dict[str, str]
    allow_dynamic_registration: bool
    allow_hot_reload: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RuntimeProfile")
        return cls(
            profile_id=as_str(field_value(raw, "profile_id"), "profile_id"),
            enabled_plugins=as_str_tuple(field_value(raw, "enabled_plugins"), "enabled_plugins"),
            bindings=as_str_dict(raw, "bindings"),
            allow_dynamic_registration=as_bool(
                field_value(raw, "allow_dynamic_registration"), "allow_dynamic_registration"
            ),
            allow_hot_reload=as_bool(field_value(raw, "allow_hot_reload"), "allow_hot_reload"),
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
            contract_surfaces=tuple_from_json(raw, "contract_surfaces", ContractSurface),
        )


RuntimeLock = RuntimeLoadPlan
