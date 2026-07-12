from __future__ import annotations

import json
import tempfile
from pathlib import Path

from mutsuki_runner_kit.contracts.codec import JsonValue
from mutsuki_runner_kit.contracts.errors import (
    ERR_CAPABILITY_EXHAUSTED,
    ERR_RESOURCE_GENERATION_MISMATCH,
    ERR_RESOURCE_LEASE_EXPIRED,
    ERR_RESOURCE_NOT_FOUND,
    RuntimeError,
)
from mutsuki_runner_kit.contracts.resource import (
    CommandBatch,
    CommandPlan,
    ExportPlan,
    LeaseToken,
    PlanReceipt,
    ReadPlan,
    ResourceAccess,
    ResourceId,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ResourceSemantic,
    SagaPlan,
    SnapshotDescriptor,
    StreamPlan,
    TransactionPlan,
    ValueRef,
    ValueStorage,
    WritePlan,
)
from mutsuki_runner_kit.resources import plans as resource_plans
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError


class FakeResourceProvider:
    def __init__(self, inline_value_max_bytes: int = 4096) -> None:
        self.inline_value_max_bytes = inline_value_max_bytes
        self._next = 0
        self._values: dict[str, tuple[ValueRef, JsonValue]] = {}
        self._resources: dict[str, tuple[ResourceRef, bytes, LeaseToken | None]] = {}
        self._root = Path(tempfile.gettempdir()) / "mutsuki-python-resource-manager"
        self._root.mkdir(parents=True, exist_ok=True)

    def pack_value(self, schema: str, value: JsonValue) -> JsonValue | ValueRef:
        encoded = json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()
        if len(encoded) <= self.inline_value_max_bytes:
            return value
        ref_id = self._id("value")
        value_ref = ValueRef(
            ref_id=ref_id,
            provider_id="python.resource",
            schema=schema,
            version=1,
            generation=1,
            size_hint=len(encoded),
            content_hash=_simple_hash(encoded),
            lifetime=ResourceLifetime.PERSISTENT,
            storage=ValueStorage.LOCAL_VALUE_STORE,
        )
        self._values[ref_id] = (value_ref, value)
        return value_ref

    def get_value(self, value_ref: ValueRef) -> JsonValue:
        stored = self._values.get(value_ref.ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"value.{value_ref.ref_id}")
        stored_ref, value = stored
        if stored_ref.generation != value_ref.generation:
            raise _resource_error(ERR_RESOURCE_GENERATION_MISMATCH, f"value.{value_ref.ref_id}")
        return value

    def create_mmap_resource(self, schema: str, data: bytes) -> ResourceRef:
        ref_id = self._id("resource")
        path = self._root / f"{ref_id}.bin"
        path.write_bytes(data)
        resource = ResourceRef(
            ref_id=ref_id,
            resource_id=_resource_id("bytes", ref_id),
            semantic=ResourceSemantic.FROZEN_VALUE,
            provider_id="python.resource",
            resource_kind="bytes",
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.mmap_file(
                path=str(path),
                offset=0,
                len=len(data),
                readonly=True,
            ),
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=ResourceLifetime.PERSISTENT,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, data)

    def create_blob_resource(self, schema: str, data: bytes) -> ResourceRef:
        ref_id = self._id("resource")
        resource = ResourceRef(
            ref_id=ref_id,
            resource_id=_resource_id("blob", ref_id),
            semantic=ResourceSemantic.FROZEN_VALUE,
            provider_id="python.resource",
            resource_kind="blob",
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.blob(store_id="python.resource.blob", key=ref_id),
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=ResourceLifetime.PERSISTENT,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, data)

    def read_resource(self, resource_ref: ResourceRef) -> bytes:
        stored = self._resources.get(resource_ref.ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"resource.{resource_ref.ref_id}")
        current, data, _lease = stored
        if current.generation != resource_ref.generation:
            raise _resource_error(
                ERR_RESOURCE_GENERATION_MISMATCH, f"resource.{resource_ref.ref_id}"
            )
        if current.access.path is not None:
            return Path(current.access.path).read_bytes()
        return data

    def copy_on_write(self, base_ref: ResourceRef, data: bytes) -> ResourceRef:
        self.read_resource(base_ref)
        return self.create_cow_state_resource(base_ref.resource_kind, base_ref.schema, data)

    def create_cow_state_resource(self, kind_id: str, schema: str, data: bytes) -> ResourceRef:
        ref_id = self._id("resource")
        path = self._root / f"{ref_id}.bin"
        path.write_bytes(data)
        resource = ResourceRef(
            ref_id=ref_id,
            resource_id=_resource_id(kind_id, ref_id),
            semantic=ResourceSemantic.COW_VERSIONED_STATE,
            provider_id="python.resource",
            resource_kind=kind_id,
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.mmap_file(
                path=str(path),
                offset=0,
                len=len(data),
                readonly=True,
            ),
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=ResourceLifetime.PERSISTENT,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, data)

    def create_fact_resource(self, kind_id: str, schema: str, value: JsonValue) -> ResourceRef:
        data = json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()
        ref_id = self._id("resource")
        resource = ResourceRef(
            ref_id=ref_id,
            resource_id=_resource_id(kind_id, ref_id),
            semantic=ResourceSemantic.READ_ONLY_FACT,
            provider_id="python.resource",
            resource_kind=kind_id,
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.provider_rpc("python.resource", "fact.read"),
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=ResourceLifetime.PERSISTENT,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, data)

    def create_stream_resource(self, kind_id: str, schema: str, endpoint: str) -> ResourceRef:
        ref_id = self._id("resource")
        resource = ResourceRef(
            ref_id=ref_id,
            resource_id=_resource_id(kind_id, ref_id),
            semantic=ResourceSemantic.STREAM_RESOURCE,
            provider_id="python.resource",
            resource_kind=kind_id,
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.stream(endpoint),
            size_hint=None,
            content_hash=None,
            lifetime=ResourceLifetime.EXTERNAL_MANAGED,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, b"")

    def create_snapshot_resource(
        self, kind_id: str, schema: str, source_ref: ResourceRef, data: bytes
    ) -> ResourceRef:
        self.read_resource(source_ref)
        ref_id = self._id("resource")
        path = self._root / f"{ref_id}.bin"
        path.write_bytes(data)
        resource = ResourceRef(
            ref_id=ref_id,
            resource_id=_resource_id(kind_id, ref_id),
            semantic=ResourceSemantic.VERSIONED_SNAPSHOT,
            provider_id="python.resource",
            resource_kind=kind_id,
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.mmap_file(
                path=str(path),
                offset=0,
                len=len(data),
                readonly=True,
            ),
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=ResourceLifetime.PERSISTENT,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, data)

    def create_capability_resource(self, kind_id: str, schema: str) -> ResourceRef:
        ref_id = self._id("resource")
        resource = ResourceRef(
            ref_id=ref_id,
            resource_id=_resource_id(kind_id, ref_id),
            semantic=ResourceSemantic.CAPABILITY_RESOURCE,
            provider_id="python.resource",
            resource_kind=kind_id,
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.provider_rpc("python.resource", "capability.command"),
            size_hint=None,
            content_hash=None,
            lifetime=ResourceLifetime.EXTERNAL_MANAGED,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, b"")

    def build_read_plan(self, resource_ref: ResourceRef, operation: str) -> ReadPlan:
        return resource_plans.build_read_plan(resource_ref, operation)

    def collect_read_plan(self, plan: ReadPlan) -> bytes:
        return resource_plans.collect_read_plan(self, plan)

    def snapshot_read_plan(self, plan: ReadPlan, kind_id: str, schema: str) -> SnapshotDescriptor:
        return resource_plans.snapshot_read_plan(self, plan, kind_id, schema)

    def open_stream_plan(self, plan: ReadPlan) -> StreamPlan:
        return resource_plans.open_stream_plan(plan)

    def export_plan(self, resource_ref: ResourceRef, target: str) -> ExportPlan:
        return resource_plans.export_plan(resource_ref, target)

    def command_plan(
        self,
        capability: ResourceRef,
        operation: str,
        args: JsonValue,
        idempotency_key: str | None = None,
    ) -> CommandPlan:
        return resource_plans.command_plan(capability, operation, args, idempotency_key)

    def execute_export_plan(self, plan: ExportPlan) -> PlanReceipt:
        return resource_plans.execute_export_plan(self, plan)

    def execute_command_plan(self, plan: CommandPlan) -> PlanReceipt:
        return resource_plans.execute_command_plan(self, plan)

    def execute_command_batch(self, batch: CommandBatch) -> tuple[PlanReceipt, ...]:
        return resource_plans.execute_command_batch(self, batch)

    def execute_saga_plan(self, saga: SagaPlan) -> tuple[PlanReceipt, ...]:
        return resource_plans.execute_saga_plan(self, saga)

    def build_write_plan(
        self, resource_ref: ResourceRef, conflict_policy: str, operations: JsonValue
    ) -> WritePlan:
        return resource_plans.build_write_plan(resource_ref, conflict_policy, operations)

    def transaction_plan(
        self, plan_id: str, operations: tuple[WritePlan, ...], strict: bool
    ) -> TransactionPlan:
        return resource_plans.transaction_plan(plan_id, operations, strict)

    def command_batch(
        self, batch_id: str, commands: tuple[CommandPlan, ...], rollback_guarantee: bool
    ) -> CommandBatch:
        return resource_plans.command_batch(batch_id, commands, rollback_guarantee)

    def saga_plan(
        self,
        saga_id: str,
        steps: tuple[CommandPlan, ...],
        compensations: tuple[CommandPlan, ...],
    ) -> SagaPlan:
        return resource_plans.saga_plan(saga_id, steps, compensations)

    def commit_write_plan(self, plan: WritePlan, data: bytes) -> PlanReceipt:
        return resource_plans.commit_write_plan(self, plan, data)

    def acquire_write_lease(
        self,
        ref_id: str,
        owner: str,
        expires_at_step: int | None = None,
    ) -> LeaseToken:
        stored = self._resources.get(ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"resource.lease.{ref_id}")
        resource, data, lease = stored
        if lease is not None:
            raise _resource_error(ERR_CAPABILITY_EXHAUSTED, f"resource.lease.{ref_id}")
        token = LeaseToken(
            token_id=self._id("lease"),
            ref_id=ref_id,
            owner=owner,
            mode="exclusive_write",
            expires_at_step=expires_at_step,
            generation=resource.generation,
        )
        self._resources[ref_id] = (resource, data, token)
        return token

    def write_with_lease(self, token: LeaseToken, data: bytes, current_step: int) -> ResourceRef:
        if token.expires_at_step is not None and current_step > token.expires_at_step:
            raise _resource_error(ERR_RESOURCE_LEASE_EXPIRED, f"resource.write.{token.ref_id}")
        stored = self._resources.get(token.ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"resource.write.{token.ref_id}")
        resource, _old_data, lease = stored
        if lease != token:
            raise _resource_error(
                ERR_RESOURCE_GENERATION_MISMATCH, f"resource.write.{token.ref_id}"
            )
        path = resource.access.path
        if path is not None:
            Path(path).write_bytes(data)
        updated = ResourceRef(
            ref_id=resource.ref_id,
            resource_id=ResourceId(
                kind_id=resource.resource_id.kind_id,
                slot_id=resource.resource_id.slot_id,
                generation=resource.generation + 1,
                version=resource.version + 1,
            ),
            semantic=resource.semantic,
            provider_id=resource.provider_id,
            resource_kind=resource.resource_kind,
            schema=resource.schema,
            version=resource.version + 1,
            generation=resource.generation + 1,
            access=resource.access,
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=resource.lifetime,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        self._resources[token.ref_id] = (updated, data, None)
        return updated

    def _id(self, prefix: str) -> str:
        self._next += 1
        return f"{prefix}-{self._next:08d}"

    def _store_resource(self, resource: ResourceRef, data: bytes) -> ResourceRef:
        self._resources[resource.ref_id] = (resource, data, None)
        return resource


def _resource_error(code: str, route: str) -> RunnerInvokeError:
    return RunnerInvokeError(RuntimeError(code=code, source="python_resource_manager", route=route))


def _resource_id(kind_id: str, ref_id: str) -> ResourceId:
    return ResourceId(kind_id=kind_id, slot_id=ref_id, generation=1, version=1)


def _simple_hash(data: bytes) -> str:
    return f"sum:{sum(data)}:len:{len(data)}"
