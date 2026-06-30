from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import TYPE_CHECKING, Self

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
    sequence,
)

if TYPE_CHECKING:
    from mutsuki_runner_kit.contracts.resource import ResourceRef


@dataclass(frozen=True)
class SnapshotDescriptor:
    snapshot_ref: ResourceRef
    source_ref: ResourceRef
    source_version: int
    snapshot_version: int
    is_stale: bool
    is_latest: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "SnapshotDescriptor")
        return cls(
            snapshot_ref=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "snapshot_ref"), "snapshot_ref")
            ),
            source_ref=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "source_ref"), "source_ref")
            ),
            source_version=as_int(field_value(raw, "source_version"), "source_version"),
            snapshot_version=as_int(field_value(raw, "snapshot_version"), "snapshot_version"),
            is_stale=as_bool(field_value(raw, "is_stale"), "is_stale"),
            is_latest=as_bool(field_value(raw, "is_latest"), "is_latest"),
        )


@dataclass(frozen=True)
class PatchDescriptor:
    patch_id: str
    target_ref: ResourceRef
    base_version: int
    conflict_policy: str
    operations: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "PatchDescriptor")
        return cls(
            patch_id=as_str(field_value(raw, "patch_id"), "patch_id"),
            target_ref=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "target_ref"), "target_ref")
            ),
            base_version=as_int(field_value(raw, "base_version"), "base_version"),
            conflict_policy=as_str(field_value(raw, "conflict_policy"), "conflict_policy"),
            operations=as_json_value(field_value(raw, "operations")),
        )


@dataclass(frozen=True)
class ReadPlan:
    plan_id: str
    resource: ResourceRef
    operation: str
    args: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "ReadPlan")
        return cls(
            plan_id=as_str(field_value(raw, "plan_id"), "plan_id"),
            resource=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "resource"), "resource")
            ),
            operation=as_str(field_value(raw, "operation"), "operation"),
            args=as_json_value(field_value(raw, "args")),
        )


@dataclass(frozen=True)
class WritePlan:
    plan_id: str
    resource: ResourceRef
    base_version: int
    conflict_policy: str
    patch: PatchDescriptor
    returning: ReadPlan | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "WritePlan")
        returning = field_value(raw, "returning")
        return cls(
            plan_id=as_str(field_value(raw, "plan_id"), "plan_id"),
            resource=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "resource"), "resource")
            ),
            base_version=as_int(field_value(raw, "base_version"), "base_version"),
            conflict_policy=as_str(field_value(raw, "conflict_policy"), "conflict_policy"),
            patch=PatchDescriptor.from_json_dict(as_mapping(field_value(raw, "patch"), "patch")),
            returning=None
            if returning is None
            else ReadPlan.from_json_dict(as_mapping(returning, "returning")),
        )


@dataclass(frozen=True)
class StreamPlan:
    plan_id: str
    resource: ResourceRef
    operation: str
    args: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "StreamPlan")
        return cls(
            plan_id=as_str(field_value(raw, "plan_id"), "plan_id"),
            resource=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "resource"), "resource")
            ),
            operation=as_str(field_value(raw, "operation"), "operation"),
            args=as_json_value(field_value(raw, "args")),
        )


@dataclass(frozen=True)
class ExportPlan:
    plan_id: str
    resource: ResourceRef
    target: str
    args: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "ExportPlan")
        return cls(
            plan_id=as_str(field_value(raw, "plan_id"), "plan_id"),
            resource=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "resource"), "resource")
            ),
            target=as_str(field_value(raw, "target"), "target"),
            args=as_json_value(field_value(raw, "args")),
        )


@dataclass(frozen=True)
class CommandPlan:
    plan_id: str
    capability: ResourceRef
    operation: str
    args: JsonValue
    idempotency_key: str | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "CommandPlan")
        return cls(
            plan_id=as_str(field_value(raw, "plan_id"), "plan_id"),
            capability=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "capability"), "capability")
            ),
            operation=as_str(field_value(raw, "operation"), "operation"),
            args=as_json_value(field_value(raw, "args")),
            idempotency_key=optional_str(field_value(raw, "idempotency_key"), "idempotency_key"),
        )


@dataclass(frozen=True)
class TransactionPlan:
    plan_id: str
    operations: tuple[WritePlan, ...]
    strict: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TransactionPlan")
        return cls(
            plan_id=as_str(field_value(raw, "plan_id"), "plan_id"),
            operations=tuple(
                WritePlan.from_json_dict(as_mapping(item, "WritePlan"))
                for item in sequence(field_value(raw, "operations"), "operations")
            ),
            strict=as_bool(field_value(raw, "strict"), "strict"),
        )


@dataclass(frozen=True)
class CommandBatch:
    batch_id: str
    commands: tuple[CommandPlan, ...]
    rollback_guarantee: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "CommandBatch")
        return cls(
            batch_id=as_str(field_value(raw, "batch_id"), "batch_id"),
            commands=tuple(
                CommandPlan.from_json_dict(as_mapping(item, "CommandPlan"))
                for item in sequence(field_value(raw, "commands"), "commands")
            ),
            rollback_guarantee=as_bool(
                field_value(raw, "rollback_guarantee"), "rollback_guarantee"
            ),
        )


@dataclass(frozen=True)
class SagaPlan:
    saga_id: str
    steps: tuple[CommandPlan, ...]
    compensations: tuple[CommandPlan, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "SagaPlan")
        return cls(
            saga_id=as_str(field_value(raw, "saga_id"), "saga_id"),
            steps=tuple(
                CommandPlan.from_json_dict(as_mapping(item, "CommandPlan"))
                for item in sequence(field_value(raw, "steps"), "steps")
            ),
            compensations=tuple(
                CommandPlan.from_json_dict(as_mapping(item, "CommandPlan"))
                for item in sequence(field_value(raw, "compensations"), "compensations")
            ),
        )


@dataclass(frozen=True)
class PlanReceipt:
    plan_id: str
    status: str
    resource_ref: ResourceRef | None
    snapshot: SnapshotDescriptor | None
    new_version: int | None
    output: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.resource import ResourceRef

        raw = as_mapping(data, "PlanReceipt")
        resource_ref = field_value(raw, "resource_ref")
        snapshot = field_value(raw, "snapshot")
        return cls(
            plan_id=as_str(field_value(raw, "plan_id"), "plan_id"),
            status=as_str(field_value(raw, "status"), "status"),
            resource_ref=None
            if resource_ref is None
            else ResourceRef.from_json_dict(as_mapping(resource_ref, "resource_ref")),
            snapshot=None
            if snapshot is None
            else SnapshotDescriptor.from_json_dict(as_mapping(snapshot, "snapshot")),
            new_version=optional_int(field_value(raw, "new_version"), "new_version"),
            output=as_json_value(field_value(raw, "output")),
        )
