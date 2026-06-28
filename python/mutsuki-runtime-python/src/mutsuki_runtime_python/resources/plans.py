from __future__ import annotations

from typing import Protocol

from mutsuki_runtime_python.contracts.codec import JsonValue
from mutsuki_runtime_python.contracts.errors import (
    ERR_RESOURCE_GENERATION_MISMATCH,
    RuntimeError,
)
from mutsuki_runtime_python.contracts.resource import (
    CommandBatch,
    CommandPlan,
    ExportPlan,
    LeaseToken,
    PatchDescriptor,
    PlanReceipt,
    ReadPlan,
    ResourceRef,
    ResourceSemantic,
    SagaPlan,
    SnapshotDescriptor,
    StreamPlan,
    TransactionPlan,
    WritePlan,
)
from mutsuki_runtime_python.runners.protocol import RunnerInvokeError


class ResourcePlanHost(Protocol):
    def read_resource(self, resource_ref: ResourceRef) -> bytes: ...

    def create_snapshot_resource(
        self, kind_id: str, schema: str, source_ref: ResourceRef, data: bytes
    ) -> ResourceRef: ...

    def acquire_write_lease(
        self, ref_id: str, owner: str, expires_at_step: int | None = None
    ) -> LeaseToken: ...

    def write_with_lease(
        self, token: LeaseToken, data: bytes, current_step: int
    ) -> ResourceRef: ...


def build_read_plan(resource_ref: ResourceRef, operation: str) -> ReadPlan:
    return ReadPlan(
        plan_id=f"read-plan:{resource_ref.ref_id}:{operation}",
        resource=resource_ref,
        operation=operation,
        args=None,
    )


def collect_read_plan(host: ResourcePlanHost, plan: ReadPlan) -> bytes:
    return host.read_resource(plan.resource)


def snapshot_read_plan(
    host: ResourcePlanHost, plan: ReadPlan, kind_id: str, schema: str
) -> SnapshotDescriptor:
    data = collect_read_plan(host, plan)
    snapshot = host.create_snapshot_resource(kind_id, schema, plan.resource, data)
    return SnapshotDescriptor(
        snapshot_ref=snapshot,
        source_ref=plan.resource,
        source_version=plan.resource.version,
        snapshot_version=snapshot.version,
        is_stale=False,
        is_latest=True,
    )


def open_stream_plan(plan: ReadPlan) -> StreamPlan:
    if plan.resource.semantic != ResourceSemantic.STREAM_RESOURCE:
        raise _resource_error(
            "resource.semantic_mismatch", f"resource.stream.{plan.resource.ref_id}"
        )
    return StreamPlan(
        plan_id=f"stream-plan:{plan.resource.ref_id}",
        resource=plan.resource,
        operation="open_stream",
        args=None,
    )


def export_plan(resource_ref: ResourceRef, target: str) -> ExportPlan:
    return ExportPlan(
        plan_id=f"export-plan:{resource_ref.ref_id}:{target}",
        resource=resource_ref,
        target=target,
        args=None,
    )


def command_plan(
    capability: ResourceRef,
    operation: str,
    args: JsonValue,
    idempotency_key: str | None = None,
) -> CommandPlan:
    return CommandPlan(
        plan_id=f"command-plan:{capability.ref_id}:{operation}",
        capability=capability,
        operation=operation,
        args=args,
        idempotency_key=idempotency_key,
    )


def build_write_plan(
    resource_ref: ResourceRef, conflict_policy: str, operations: JsonValue
) -> WritePlan:
    patch = PatchDescriptor(
        patch_id=f"patch:{resource_ref.ref_id}:{resource_ref.version}",
        target_ref=resource_ref,
        base_version=resource_ref.version,
        conflict_policy=conflict_policy,
        operations=operations,
    )
    return WritePlan(
        plan_id=f"write-plan:{resource_ref.ref_id}:{resource_ref.version}",
        resource=resource_ref,
        base_version=resource_ref.version,
        conflict_policy=conflict_policy,
        patch=patch,
        returning=None,
    )


def transaction_plan(
    plan_id: str, operations: tuple[WritePlan, ...], strict: bool
) -> TransactionPlan:
    return TransactionPlan(plan_id=plan_id, operations=operations, strict=strict)


def command_batch(
    batch_id: str, commands: tuple[CommandPlan, ...], rollback_guarantee: bool
) -> CommandBatch:
    return CommandBatch(
        batch_id=batch_id,
        commands=commands,
        rollback_guarantee=rollback_guarantee,
    )


def saga_plan(
    saga_id: str, steps: tuple[CommandPlan, ...], compensations: tuple[CommandPlan, ...]
) -> SagaPlan:
    return SagaPlan(saga_id=saga_id, steps=steps, compensations=compensations)


def commit_write_plan(host: ResourcePlanHost, plan: WritePlan, data: bytes) -> PlanReceipt:
    if plan.resource.semantic != ResourceSemantic.COW_VERSIONED_STATE:
        raise _resource_error("resource.semantic_mismatch", f"resource.plan.{plan.resource.ref_id}")
    if plan.resource.version != plan.base_version:
        raise _resource_error(
            ERR_RESOURCE_GENERATION_MISMATCH, f"resource.plan.{plan.resource.ref_id}"
        )
    lease = host.acquire_write_lease(plan.resource.ref_id, "resource-plan")
    updated = host.write_with_lease(lease, data, current_step=0)
    return PlanReceipt(
        plan_id=plan.plan_id,
        status="committed",
        resource_ref=updated,
        snapshot=None,
        new_version=updated.version,
        output=None,
    )


def _resource_error(code: str, route: str) -> RunnerInvokeError:
    return RunnerInvokeError(RuntimeError(code=code, source="python_resource_manager", route=route))
