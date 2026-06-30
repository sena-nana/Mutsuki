from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import ClassVar, Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    as_bool,
    as_int,
    as_json_value,
    as_mapping,
    as_str,
    field_value,
    optional_int,
    optional_str,
    required_optional,
    sequence,
    to_json_dict,
    to_json_value,
)


class ValueStorage(StrEnum):
    INLINE_SMALL = "inline_small"
    LOCAL_VALUE_STORE = "local_value_store"
    BLOB = "blob"
    STREAM = "stream"
    PROVIDER_RPC = "provider_rpc"


class ResourceSealState(StrEnum):
    WRITABLE = "writable"
    SEALED = "sealed"


class ResourceSemantic(StrEnum):
    FROZEN_VALUE = "frozen_value"
    VERSIONED_SNAPSHOT = "versioned_snapshot"
    READ_ONLY_FACT = "read_only_fact"
    COW_VERSIONED_STATE = "cow_versioned_state"
    CAPABILITY_RESOURCE = "capability_resource"
    STREAM_RESOURCE = "stream_resource"
    TRANSACTION_RESOURCE = "transaction_resource"


class ResourceProviderReloadPolicy(StrEnum):
    NO_LIVE_RESOURCES = "no_live_resources"
    COMPATIBLE_WITHOUT_LEASES = "compatible_without_leases"
    DRAIN_ACTIVE_LEASES = "drain_active_leases"
    RESTART_REQUIRED = "restart_required"


@dataclass(frozen=True)
class ResourceId:
    kind_id: str
    slot_id: str
    generation: int
    version: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceId")
        return cls(
            kind_id=as_str(field_value(raw, "kind_id"), "kind_id"),
            slot_id=as_str(field_value(raw, "slot_id"), "slot_id"),
            generation=as_int(field_value(raw, "generation"), "generation"),
            version=as_int(field_value(raw, "version"), "version"),
        )


@dataclass(frozen=True)
class ResourceProviderCompatibility:
    schema_version: str
    required_operations: tuple[str, ...]
    preserves_resource_type_id: bool
    accepts_older_generations: bool
    lease_drain_required: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceProviderCompatibility")
        return cls(
            schema_version=as_str(field_value(raw, "schema_version"), "schema_version"),
            required_operations=tuple(
                as_str(item, "required_operations")
                for item in sequence(field_value(raw, "required_operations"), "required_operations")
            ),
            preserves_resource_type_id=as_bool(
                field_value(raw, "preserves_resource_type_id"), "preserves_resource_type_id"
            ),
            accepts_older_generations=as_bool(
                field_value(raw, "accepts_older_generations"), "accepts_older_generations"
            ),
            lease_drain_required=as_bool(
                field_value(raw, "lease_drain_required"), "lease_drain_required"
            ),
        )


@dataclass(frozen=True)
class ResourceTypeDescriptor:
    kind_id: str
    semantic: ResourceSemantic
    schema: str
    provider_id: str
    operations: tuple[str, ...]
    reload_policy: ResourceProviderReloadPolicy
    compatibility: ResourceProviderCompatibility

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceTypeDescriptor")
        return cls(
            kind_id=as_str(field_value(raw, "kind_id"), "kind_id"),
            semantic=ResourceSemantic(as_str(field_value(raw, "semantic"), "semantic")),
            schema=as_str(field_value(raw, "schema"), "schema"),
            provider_id=as_str(field_value(raw, "provider_id"), "provider_id"),
            operations=tuple(
                as_str(item, "operations")
                for item in sequence(field_value(raw, "operations"), "operations")
            ),
            reload_policy=ResourceProviderReloadPolicy(
                as_str(field_value(raw, "reload_policy"), "reload_policy")
            ),
            compatibility=ResourceProviderCompatibility.from_json_dict(
                as_mapping(field_value(raw, "compatibility"), "compatibility")
            ),
        )


@dataclass(frozen=True)
class ResourceLifetime:
    kind: str
    lease_until_step: int | None = None

    BORROWED_UNTIL_TASK_END: ClassVar[ResourceLifetime]
    PERSISTENT: ClassVar[ResourceLifetime]
    EXTERNAL_MANAGED: ClassVar[ResourceLifetime]

    @classmethod
    def lease_until(cls, step: int) -> Self:
        return cls("lease_until", step)

    @classmethod
    def from_json_value(cls, value: object) -> Self:
        if isinstance(value, str):
            if value == "borrowed_until_task_end":
                return cls("borrowed_until_task_end")
            if value == "persistent":
                return cls("persistent")
            if value == "external_managed":
                return cls("external_managed")
            raise ValueError(f"unknown resource lifetime: {value}")
        raw = as_mapping(value, "ResourceLifetime")
        if set(raw.keys()) != {"lease_until"}:
            raise TypeError("ResourceLifetime expects a unit string or {'lease_until': step}")
        return cls.lease_until(as_int(raw["lease_until"], "lease_until"))

    def to_json_value(self) -> JsonValue:
        if self.kind == "lease_until":
            if self.lease_until_step is None:
                raise TypeError("lease_until lifetime requires lease_until_step")
            return {"lease_until": self.lease_until_step}
        if self.lease_until_step is not None:
            raise TypeError("unit lifetime cannot carry lease_until_step")
        return self.kind


ResourceLifetime.BORROWED_UNTIL_TASK_END = ResourceLifetime("borrowed_until_task_end")
ResourceLifetime.PERSISTENT = ResourceLifetime("persistent")
ResourceLifetime.EXTERNAL_MANAGED = ResourceLifetime("external_managed")


@dataclass(frozen=True)
class ResourceAccess:
    type: str
    path: str | None = None
    name: str | None = None
    offset: int | None = None
    len: int | None = None
    readonly: bool | None = None
    store_id: str | None = None
    key: str | None = None
    endpoint: str | None = None
    provider_id: str | None = None
    method: str | None = None

    @classmethod
    def inline(cls) -> Self:
        return cls(type="inline")

    @classmethod
    def mmap_file(cls, path: str, offset: int, len: int, readonly: bool) -> Self:
        return cls(type="mmap_file", path=path, offset=offset, len=len, readonly=readonly)

    @classmethod
    def shared_memory(cls, name: str, offset: int, len: int, readonly: bool) -> Self:
        return cls(type="shared_memory", name=name, offset=offset, len=len, readonly=readonly)

    @classmethod
    def blob(cls, store_id: str, key: str) -> Self:
        return cls(type="blob", store_id=store_id, key=key)

    @classmethod
    def stream(cls, endpoint: str) -> Self:
        return cls(type="stream", endpoint=endpoint)

    @classmethod
    def provider_rpc(cls, provider_id: str, method: str) -> Self:
        return cls(type="provider_rpc", provider_id=provider_id, method=method)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceAccess")
        access_type = as_str(field_value(raw, "type"), "type")
        if access_type == "inline":
            return cls.inline()
        if access_type == "mmap_file":
            return cls.mmap_file(
                path=as_str(field_value(raw, "path"), "path"),
                offset=as_int(field_value(raw, "offset"), "offset"),
                len=as_int(field_value(raw, "len"), "len"),
                readonly=as_bool(field_value(raw, "readonly"), "readonly"),
            )
        if access_type == "shared_memory":
            return cls.shared_memory(
                name=as_str(field_value(raw, "name"), "name"),
                offset=as_int(field_value(raw, "offset"), "offset"),
                len=as_int(field_value(raw, "len"), "len"),
                readonly=as_bool(field_value(raw, "readonly"), "readonly"),
            )
        if access_type == "blob":
            return cls.blob(
                store_id=as_str(field_value(raw, "store_id"), "store_id"),
                key=as_str(field_value(raw, "key"), "key"),
            )
        if access_type == "stream":
            return cls.stream(endpoint=as_str(field_value(raw, "endpoint"), "endpoint"))
        if access_type == "provider_rpc":
            return cls.provider_rpc(
                provider_id=as_str(field_value(raw, "provider_id"), "provider_id"),
                method=as_str(field_value(raw, "method"), "method"),
            )
        raise ValueError(f"unknown resource access type: {access_type}")

    def to_json_value(self) -> JsonDict:
        if self.type == "inline":
            return {"type": "inline"}
        if self.type == "mmap_file":
            return {
                "type": "mmap_file",
                "path": required_optional(self.path, "path"),
                "offset": required_optional(self.offset, "offset"),
                "len": required_optional(self.len, "len"),
                "readonly": required_optional(self.readonly, "readonly"),
            }
        if self.type == "shared_memory":
            return {
                "type": "shared_memory",
                "name": required_optional(self.name, "name"),
                "offset": required_optional(self.offset, "offset"),
                "len": required_optional(self.len, "len"),
                "readonly": required_optional(self.readonly, "readonly"),
            }
        if self.type == "blob":
            return {
                "type": "blob",
                "store_id": required_optional(self.store_id, "store_id"),
                "key": required_optional(self.key, "key"),
            }
        if self.type == "stream":
            return {"type": "stream", "endpoint": required_optional(self.endpoint, "endpoint")}
        if self.type == "provider_rpc":
            return {
                "type": "provider_rpc",
                "provider_id": required_optional(self.provider_id, "provider_id"),
                "method": required_optional(self.method, "method"),
            }
        raise ValueError(f"unknown resource access type: {self.type}")


@dataclass(frozen=True)
class LeaseToken:
    token_id: str
    ref_id: str
    owner: str
    mode: str
    expires_at_step: int | None
    generation: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "LeaseToken")
        return cls(
            token_id=as_str(field_value(raw, "token_id"), "token_id"),
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            owner=as_str(field_value(raw, "owner"), "owner"),
            mode=as_str(field_value(raw, "mode"), "mode"),
            expires_at_step=optional_int(field_value(raw, "expires_at_step"), "expires_at_step"),
            generation=as_int(field_value(raw, "generation"), "generation"),
        )


@dataclass(frozen=True)
class ExclusiveWriteLease:
    token: LeaseToken

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ExclusiveWriteLease")
        return cls(token=LeaseToken.from_json_dict(as_mapping(field_value(raw, "token"), "token")))


@dataclass(frozen=True)
class ResourceCellRef:
    cell_id: str
    resource_kind: str
    owner_plugin_id: str
    schema: str
    generation: int
    health: str
    reload_policy: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceCellRef")
        return cls(
            cell_id=as_str(field_value(raw, "cell_id"), "cell_id"),
            resource_kind=as_str(field_value(raw, "resource_kind"), "resource_kind"),
            owner_plugin_id=as_str(field_value(raw, "owner_plugin_id"), "owner_plugin_id"),
            schema=as_str(field_value(raw, "schema"), "schema"),
            generation=as_int(field_value(raw, "generation"), "generation"),
            health=as_str(field_value(raw, "health"), "health"),
            reload_policy=as_str(field_value(raw, "reload_policy"), "reload_policy"),
        )


@dataclass(frozen=True)
class ResourceLease:
    lease_id: str
    cell_id: str
    borrower_task_id: str
    borrower_executor_id: str
    mode: str
    expires_at_step: int | None
    generation: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceLease")
        return cls(
            lease_id=as_str(field_value(raw, "lease_id"), "lease_id"),
            cell_id=as_str(field_value(raw, "cell_id"), "cell_id"),
            borrower_task_id=as_str(field_value(raw, "borrower_task_id"), "borrower_task_id"),
            borrower_executor_id=as_str(
                field_value(raw, "borrower_executor_id"), "borrower_executor_id"
            ),
            mode=as_str(field_value(raw, "mode"), "mode"),
            expires_at_step=optional_int(field_value(raw, "expires_at_step"), "expires_at_step"),
            generation=as_int(field_value(raw, "generation"), "generation"),
        )


@dataclass(frozen=True)
class ResourceRef:
    ref_id: str
    resource_id: ResourceId
    semantic: ResourceSemantic
    provider_id: str
    resource_kind: str
    schema: str
    version: int
    generation: int
    access: ResourceAccess
    size_hint: int | None
    content_hash: str | None
    lifetime: ResourceLifetime
    lease: LeaseToken | None
    seal_state: ResourceSealState

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceRef")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            resource_id=ResourceId.from_json_dict(
                as_mapping(field_value(raw, "resource_id"), "resource_id")
            ),
            semantic=ResourceSemantic(as_str(field_value(raw, "semantic"), "semantic")),
            provider_id=as_str(field_value(raw, "provider_id"), "provider_id"),
            resource_kind=as_str(field_value(raw, "resource_kind"), "resource_kind"),
            schema=as_str(field_value(raw, "schema"), "schema"),
            version=as_int(field_value(raw, "version"), "version"),
            generation=as_int(field_value(raw, "generation"), "generation"),
            access=ResourceAccess.from_json_dict(as_mapping(field_value(raw, "access"), "access")),
            size_hint=optional_int(field_value(raw, "size_hint"), "size_hint"),
            content_hash=optional_str(field_value(raw, "content_hash"), "content_hash"),
            lifetime=ResourceLifetime.from_json_value(field_value(raw, "lifetime")),
            lease=None
            if field_value(raw, "lease") is None
            else LeaseToken.from_json_dict(as_mapping(field_value(raw, "lease"), "lease")),
            seal_state=ResourceSealState(as_str(field_value(raw, "seal_state"), "seal_state")),
        )


@dataclass(frozen=True)
class ValueRef:
    ref_id: str
    provider_id: str
    schema: str
    version: int
    generation: int
    size_hint: int | None
    content_hash: str | None
    lifetime: ResourceLifetime
    storage: ValueStorage

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ValueRef")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            provider_id=as_str(field_value(raw, "provider_id"), "provider_id"),
            schema=as_str(field_value(raw, "schema"), "schema"),
            version=as_int(field_value(raw, "version"), "version"),
            generation=as_int(field_value(raw, "generation"), "generation"),
            size_hint=optional_int(field_value(raw, "size_hint"), "size_hint"),
            content_hash=optional_str(field_value(raw, "content_hash"), "content_hash"),
            lifetime=ResourceLifetime.from_json_value(field_value(raw, "lifetime")),
            storage=ValueStorage(as_str(field_value(raw, "storage"), "storage")),
        )


from mutsuki_runner_kit.contracts.resource_plans import (  # noqa: E402
    CommandBatch,
    CommandPlan,
    ExportPlan,
    PatchDescriptor,
    PlanReceipt,
    ReadPlan,
    SagaPlan,
    SnapshotDescriptor,
    StreamPlan,
    TransactionPlan,
    WritePlan,
)

__all__ = (
    "CommandBatch",
    "CommandPlan",
    "ExclusiveWriteLease",
    "ExportPlan",
    "LeaseToken",
    "PatchDescriptor",
    "PlanReceipt",
    "ReadPlan",
    "ResourceAccess",
    "ResourceCellRef",
    "ResourceId",
    "ResourceLease",
    "ResourceLifetime",
    "ResourceProviderCompatibility",
    "ResourceProviderReloadPolicy",
    "ResourceRef",
    "ResourceSealState",
    "ResourceSemantic",
    "ResourceTypeDescriptor",
    "ResourceValue",
    "SagaPlan",
    "SnapshotDescriptor",
    "StreamPlan",
    "TransactionPlan",
    "ValueRef",
    "ValueStorage",
    "WritePlan",
)


@dataclass(frozen=True)
class ResourceValue:
    type: str
    schema: str | None = None
    value: JsonValue = None
    version: int | None = None
    value_ref: ValueRef | None = None
    resource_ref: ResourceRef | None = None

    @classmethod
    def inline(cls, schema: str, value: JsonValue, version: int) -> Self:
        return cls(type="inline", schema=schema, value=value, version=version)

    @classmethod
    def value_ref_value(cls, value_ref: ValueRef) -> Self:
        return cls(type="value_ref", value_ref=value_ref)

    @classmethod
    def resource_ref_value(cls, resource_ref: ResourceRef) -> Self:
        return cls(type="resource_ref", resource_ref=resource_ref)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceValue")
        value_type = as_str(field_value(raw, "type"), "type")
        if value_type == "inline":
            return cls.inline(
                schema=as_str(field_value(raw, "schema"), "schema"),
                value=as_json_value(field_value(raw, "value")),
                version=as_int(field_value(raw, "version"), "version"),
            )
        if value_type == "value_ref":
            return cls.value_ref_value(ValueRef.from_json_dict(raw))
        if value_type == "resource_ref":
            return cls.resource_ref_value(ResourceRef.from_json_dict(raw))
        raise ValueError(f"unknown resource value type: {value_type}")

    def to_json_value(self) -> JsonDict:
        if self.type == "inline":
            return {
                "type": "inline",
                "schema": required_optional(self.schema, "schema"),
                "value": to_json_value(self.value),
                "version": required_optional(self.version, "version"),
            }
        if self.type == "value_ref":
            value_ref = required_optional(self.value_ref, "value_ref")
            return {"type": "value_ref", **to_json_dict(value_ref)}
        if self.type == "resource_ref":
            resource_ref = required_optional(self.resource_ref, "resource_ref")
            return {"type": "resource_ref", **to_json_dict(resource_ref)}
        raise ValueError(f"unknown resource value type: {self.type}")
