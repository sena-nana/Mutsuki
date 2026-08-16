from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import ClassVar, Protocol, TypeVar

from mutsuki_runner_kit.contracts.codec import JsonValue
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.resource import (
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
from mutsuki_runner_kit.resources import plans as resource_plans
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError


class ResourceKind(Protocol):
    KIND_ID: ClassVar[str]
    SEMANTIC: ClassVar[ResourceSemantic]


TResourceKind = TypeVar("TResourceKind", bound=ResourceKind)

_SDK_SOURCE = "runtime.sdk"

_READ_PLAN_SEMANTICS = frozenset(
    {
        ResourceSemantic.FROZEN_VALUE,
        ResourceSemantic.VERSIONED_SNAPSHOT,
        ResourceSemantic.READ_ONLY_FACT,
        ResourceSemantic.COW_VERSIONED_STATE,
        ResourceSemantic.STREAM_RESOURCE,
    }
)
_WRITE_PLAN_SEMANTICS = frozenset({ResourceSemantic.COW_VERSIONED_STATE})
_STREAM_PLAN_SEMANTICS = frozenset({ResourceSemantic.STREAM_RESOURCE})
_EXPORT_PLAN_SEMANTICS = frozenset(
    {
        ResourceSemantic.FROZEN_VALUE,
        ResourceSemantic.VERSIONED_SNAPSHOT,
        ResourceSemantic.READ_ONLY_FACT,
        ResourceSemantic.COW_VERSIONED_STATE,
    }
)
_COMMAND_PLAN_SEMANTICS = frozenset({ResourceSemantic.CAPABILITY_RESOURCE})


@dataclass(frozen=True)
class TypedResourceHandle[TResourceKind: ResourceKind]:
    resource: ResourceRef
    kind: type[TResourceKind]

    def __post_init__(self) -> None:
        _require_kind_and_semantic(self.resource, self.kind)

    def into_resource(self) -> ResourceRef:
        return self.resource


class ResourceClient:
    def handle(
        self, resource: ResourceRef, kind: type[TResourceKind]
    ) -> TypedResourceHandle[TResourceKind]:
        return TypedResourceHandle(resource=resource, kind=kind)

    def read_plan(self, handle: TypedResourceHandle[TResourceKind], operation: str) -> ReadPlan:
        _require_plan_semantic(
            handle.resource,
            _READ_PLAN_SEMANTICS,
            f"resource.read_plan.{handle.resource.ref_id}",
        )
        return resource_plans.build_read_plan(handle.resource, operation)

    def write_plan(
        self,
        handle: TypedResourceHandle[TResourceKind],
        conflict_policy: str,
        operations: JsonValue,
    ) -> WritePlan:
        _require_plan_semantic(
            handle.resource,
            _WRITE_PLAN_SEMANTICS,
            f"resource.write_plan.{handle.resource.ref_id}",
        )
        return resource_plans.build_write_plan(handle.resource, conflict_policy, operations)

    def stream_plan(self, handle: TypedResourceHandle[TResourceKind]) -> StreamPlan:
        _require_plan_semantic(
            handle.resource,
            _STREAM_PLAN_SEMANTICS,
            f"resource.stream_plan.{handle.resource.ref_id}",
        )
        return resource_plans.open_stream_plan(
            resource_plans.build_read_plan(handle.resource, "open_stream")
        )

    def export_plan(self, handle: TypedResourceHandle[TResourceKind], target: str) -> ExportPlan:
        _require_plan_semantic(
            handle.resource,
            _EXPORT_PLAN_SEMANTICS,
            f"resource.export_plan.{handle.resource.ref_id}",
        )
        return resource_plans.export_plan(handle.resource, target)

    def command_plan(
        self,
        capability: TypedResourceHandle[TResourceKind],
        operation: str,
        args: JsonValue,
        idempotency_key: str | None = None,
    ) -> CommandPlan:
        _require_plan_semantic(
            capability.resource,
            _COMMAND_PLAN_SEMANTICS,
            f"resource.command_plan.{capability.resource.ref_id}",
        )
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


def _require_kind_and_semantic(resource: ResourceRef, kind: type[ResourceKind]) -> None:
    if resource.resource_id.kind_id != kind.KIND_ID:
        raise _sdk_error("resource.kind_mismatch", f"resource.handle.{resource.ref_id}")
    if resource.semantic != kind.SEMANTIC:
        raise _sdk_error("resource.semantic_mismatch", f"resource.handle.{resource.ref_id}")


def _require_plan_semantic(
    resource: ResourceRef, allowed: frozenset[ResourceSemantic], route: str
) -> None:
    if resource.semantic not in allowed:
        raise _sdk_error("resource.semantic_mismatch", route)


def _sdk_error(code: str, route: str) -> RunnerInvokeError:
    return RunnerInvokeError(RuntimeError(code=code, source=_SDK_SOURCE, route=route))


__all__ = (
    "ResourceClient",
    "ResourceKind",
    "TypedResourceHandle",
)
