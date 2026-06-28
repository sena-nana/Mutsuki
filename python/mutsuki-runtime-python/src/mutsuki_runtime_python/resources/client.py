from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import ClassVar, Protocol, TypeVar

from mutsuki_runtime_python.contracts.codec import JsonValue
from mutsuki_runtime_python.contracts.resource import (
    CommandBatch,
    CommandPlan,
    ExportPlan,
    ReadPlan,
    ResourceRef,
    ResourceSemantic,
    SagaPlan,
    StreamPlan,
    TransactionPlan,
    WritePlan,
)
from mutsuki_runtime_python.resources import plans as resource_plans


class ResourceKind(Protocol):
    KIND_ID: ClassVar[str]
    SEMANTIC: ClassVar[ResourceSemantic]


class TextBuffer:
    KIND_ID: ClassVar[str] = "text_buffer"
    SEMANTIC: ClassVar[ResourceSemantic] = ResourceSemantic.COW_VERSIONED_STATE


class AstSnapshot:
    KIND_ID: ClassVar[str] = "ast_snapshot"
    SEMANTIC: ClassVar[ResourceSemantic] = ResourceSemantic.VERSIONED_SNAPSHOT


class ProjectFacts:
    KIND_ID: ClassVar[str] = "project_facts"
    SEMANTIC: ClassVar[ResourceSemantic] = ResourceSemantic.READ_ONLY_FACT


class ModelOutputStream:
    KIND_ID: ClassVar[str] = "model_output_stream"
    SEMANTIC: ClassVar[ResourceSemantic] = ResourceSemantic.STREAM_RESOURCE


class DbPool:
    KIND_ID: ClassVar[str] = "db_pool"
    SEMANTIC: ClassVar[ResourceSemantic] = ResourceSemantic.CAPABILITY_RESOURCE


TResourceKind = TypeVar("TResourceKind", bound=ResourceKind)


@dataclass(frozen=True)
class TypedResourceHandle[TResourceKind: ResourceKind]:
    resource: ResourceRef
    kind: type[TResourceKind]

    def descriptor_matches_kind(self) -> bool:
        return (
            self.resource.resource_id.kind_id == self.kind.KIND_ID
            and self.resource.semantic == self.kind.SEMANTIC
        )


class ResourceClient:
    def handle(
        self, resource: ResourceRef, kind: type[TResourceKind]
    ) -> TypedResourceHandle[TResourceKind]:
        return TypedResourceHandle(resource=resource, kind=kind)

    def read_plan(
        self, handle: TypedResourceHandle[TResourceKind], operation: str
    ) -> ReadPlan:
        return resource_plans.build_read_plan(handle.resource, operation)

    def write_plan(
        self,
        handle: TypedResourceHandle[TResourceKind],
        conflict_policy: str,
        operations: JsonValue,
    ) -> WritePlan:
        return resource_plans.build_write_plan(handle.resource, conflict_policy, operations)

    def stream_plan(self, handle: TypedResourceHandle[TResourceKind]) -> StreamPlan:
        return resource_plans.open_stream_plan(
            resource_plans.build_read_plan(handle.resource, "open_stream")
        )

    def export_plan(
        self, handle: TypedResourceHandle[TResourceKind], target: str
    ) -> ExportPlan:
        return resource_plans.export_plan(handle.resource, target)

    def command_plan(
        self,
        capability: TypedResourceHandle[TResourceKind],
        operation: str,
        args: JsonValue,
        idempotency_key: str | None = None,
    ) -> CommandPlan:
        return resource_plans.command_plan(
            capability.resource,
            operation,
            args,
            idempotency_key,
        )

    def transaction_plan(
        self, plan_id: str, operations: Sequence[WritePlan], strict: bool
    ) -> TransactionPlan:
        return resource_plans.transaction_plan(plan_id, tuple(operations), strict)

    def command_batch(
        self,
        batch_id: str,
        commands: Sequence[CommandPlan],
        rollback_guarantee: bool,
    ) -> CommandBatch:
        return resource_plans.command_batch(batch_id, tuple(commands), rollback_guarantee)

    def saga_plan(
        self,
        saga_id: str,
        steps: Sequence[CommandPlan],
        compensations: Sequence[CommandPlan],
    ) -> SagaPlan:
        return resource_plans.saga_plan(saga_id, tuple(steps), tuple(compensations))


__all__ = (
    "AstSnapshot",
    "DbPool",
    "ModelOutputStream",
    "ProjectFacts",
    "ResourceClient",
    "ResourceKind",
    "TextBuffer",
    "TypedResourceHandle",
)
