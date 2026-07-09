from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    ScalarValue,
    as_int,
    as_json_value,
    as_mapping,
    as_scalar,
    as_str,
    as_str_tuple,
    field_value,
    optional_int,
    optional_str,
    sequence,
    to_json_value,
    tuple_from_json,
)
from mutsuki_runner_kit.contracts.entry import (
    DispatchLane,
    OrderingRequirement,
    PayloadLayout,
    ResourceAccessMode,
    ResourceRequirement,
)
from mutsuki_runner_kit.contracts.errors import ERR_TASK_CLAIM_CONFLICT, RuntimeError
from mutsuki_runner_kit.contracts.resource import ResourceRef
from mutsuki_runner_kit.contracts.state import VersionExpectation
from mutsuki_runner_kit.contracts.task import Task, TaskLease

if TYPE_CHECKING:
    from mutsuki_runner_kit.contracts.runner import RunnerResult

__all__ = (
    "BatchEntry",
    "BatchPayload",
    "BinaryPackedPayload",
    "ColumnPayload",
    "ColumnarPayload",
    "CompletionBatch",
    "DeferredResourceOp",
    "DispatchLane",
    "EntryCompletion",
    "OrderingRequirement",
    "PayloadLayout",
    "ResourceAccessMode",
    "ResourceBackedPayload",
    "ResourceReadView",
    "ResourceRequirement",
    "ResourceSlice",
    "ResourceWriteLock",
    "RowPayload",
    "TaskBatch",
    "WorkBatch",
    "WorkResourcePlan",
    "WorkSet",
)


@dataclass(frozen=True)
class BatchEntry:
    entry_id: str
    task_id: str
    trace_id: str | None
    parent_id: str | None
    payload_index: int
    resource_requirement_indices: tuple[int, ...]
    cancel_index: int | None
    deadline_tick: int | None
    priority: int
    lane: DispatchLane
    ordering: OrderingRequirement

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "BatchEntry")
        return cls(
            entry_id=as_str(field_value(raw, "entry_id"), "entry_id"),
            task_id=as_str(field_value(raw, "task_id"), "task_id"),
            trace_id=optional_str(field_value(raw, "trace_id"), "trace_id"),
            parent_id=optional_str(field_value(raw, "parent_id"), "parent_id"),
            payload_index=as_int(field_value(raw, "payload_index"), "payload_index"),
            resource_requirement_indices=_int_tuple(
                field_value(raw, "resource_requirement_indices"),
                "resource_requirement_indices",
            ),
            cancel_index=optional_int(field_value(raw, "cancel_index"), "cancel_index"),
            deadline_tick=optional_int(field_value(raw, "deadline_tick"), "deadline_tick"),
            priority=as_int(field_value(raw, "priority"), "priority"),
            lane=DispatchLane(as_str(field_value(raw, "lane"), "lane")),
            ordering=OrderingRequirement.from_json_dict(
                as_mapping(field_value(raw, "ordering"), "ordering")
            ),
        )


@dataclass(frozen=True)
class RowPayload:
    rows: tuple[JsonValue, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RowPayload")
        return cls(
            rows=tuple(
                as_json_value(item) for item in sequence(field_value(raw, "rows"), "rows")
            )
        )


@dataclass(frozen=True)
class ColumnPayload:
    name: str
    values: tuple[JsonValue, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ColumnPayload")
        return cls(
            name=as_str(field_value(raw, "name"), "name"),
            values=tuple(
                as_json_value(item) for item in sequence(field_value(raw, "values"), "values")
            ),
        )


@dataclass(frozen=True)
class ColumnarPayload:
    columns: tuple[ColumnPayload, ...]
    row_count: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ColumnarPayload")
        return cls(
            columns=tuple_from_json(raw, "columns", ColumnPayload),
            row_count=as_int(field_value(raw, "row_count"), "row_count"),
        )


@dataclass(frozen=True)
class BinaryPackedPayload:
    encoding: str
    bytes: tuple[int, ...]
    row_count: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "BinaryPackedPayload")
        return cls(
            encoding=as_str(field_value(raw, "encoding"), "encoding"),
            bytes=_byte_tuple(field_value(raw, "bytes"), "bytes"),
            row_count=as_int(field_value(raw, "row_count"), "row_count"),
        )


@dataclass(frozen=True)
class ResourceSlice:
    resource: ResourceRef
    offset: int
    length: int | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceSlice")
        return cls(
            resource=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "resource"), "resource")
            ),
            offset=as_int(field_value(raw, "offset"), "offset"),
            length=optional_int(field_value(raw, "length"), "length"),
        )


@dataclass(frozen=True)
class ResourceBackedPayload:
    slices: tuple[ResourceSlice, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceBackedPayload")
        return cls(slices=tuple_from_json(raw, "slices", ResourceSlice))


@dataclass(frozen=True)
class BatchPayload:
    layout: PayloadLayout
    payload: RowPayload | ColumnarPayload | BinaryPackedPayload | ResourceBackedPayload

    @classmethod
    def row(cls, rows: Sequence[JsonValue]) -> Self:
        return cls(layout=PayloadLayout.ROW, payload=RowPayload(rows=tuple(rows)))

    @classmethod
    def from_tasks(cls, tasks: Sequence[Task]) -> Self:
        return cls.row([to_json_value(task) for task in tasks])

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "BatchPayload")
        layout = PayloadLayout(as_str(field_value(raw, "layout"), "layout"))
        payload_raw = as_mapping(field_value(raw, "payload"), "payload")
        if layout == PayloadLayout.ROW:
            return cls(layout=layout, payload=RowPayload.from_json_dict(payload_raw))
        if layout == PayloadLayout.COLUMNAR:
            return cls(layout=layout, payload=ColumnarPayload.from_json_dict(payload_raw))
        if layout == PayloadLayout.BINARY_PACKED:
            return cls(layout=layout, payload=BinaryPackedPayload.from_json_dict(payload_raw))
        return cls(layout=layout, payload=ResourceBackedPayload.from_json_dict(payload_raw))

    def to_json_value(self) -> JsonDict:
        return {
            "layout": self.layout.value,
            "payload": to_json_value(self.payload),
        }

    def row_count(self) -> int:
        if isinstance(self.payload, RowPayload):
            return len(self.payload.rows)
        if isinstance(self.payload, ColumnarPayload | BinaryPackedPayload):
            return self.payload.row_count
        return len(self.payload.slices)

    def try_row_tasks(self) -> tuple[Task, ...] | RuntimeError:
        if self.layout != PayloadLayout.ROW or not isinstance(self.payload, RowPayload):
            return RuntimeError(
                code=ERR_TASK_CLAIM_CONFLICT,
                source="runtime.batch_payload",
                route=f"payload.layout.{self.layout.value}",
            )
        tasks: list[Task] = []
        for index, value in enumerate(self.payload.rows):
            try:
                tasks.append(Task.from_json_dict(as_mapping(value, f"payload.row.{index}")))
            except TypeError as exc:
                return RuntimeError(
                    code=ERR_TASK_CLAIM_CONFLICT,
                    source="runtime.batch_payload",
                    route=f"payload.row.{index}",
                    evidence={"exception_repr": repr(exc)},
                )
        return tuple(tasks)


@dataclass(frozen=True)
class ResourceReadView:
    ref_id: str
    requirement_indices: tuple[int, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceReadView")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            requirement_indices=_int_tuple(
                field_value(raw, "requirement_indices"), "requirement_indices"
            ),
        )


@dataclass(frozen=True)
class ResourceWriteLock:
    ref_id: str
    requirement_indices: tuple[int, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceWriteLock")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            requirement_indices=_int_tuple(
                field_value(raw, "requirement_indices"), "requirement_indices"
            ),
        )


@dataclass(frozen=True)
class DeferredResourceOp:
    entry_id: str
    ref_id: str
    operation: str
    payload: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "DeferredResourceOp")
        return cls(
            entry_id=as_str(field_value(raw, "entry_id"), "entry_id"),
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            operation=as_str(field_value(raw, "operation"), "operation"),
            payload=as_json_value(field_value(raw, "payload")),
        )


@dataclass(frozen=True)
class WorkResourcePlan:
    read_views: tuple[ResourceReadView, ...] = ()
    write_locks: tuple[ResourceWriteLock, ...] = ()
    parallel_groups: tuple[tuple[str, ...], ...] = ()
    serial_groups: tuple[tuple[str, ...], ...] = ()
    parallelism_limit: int = 1
    version_checks: tuple[VersionExpectation, ...] = ()
    deferred_writes: tuple[DeferredResourceOp, ...] = ()
    conflict_entries: tuple[str, ...] = ()

    @classmethod
    def empty(cls) -> Self:
        return cls()

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "WorkResourcePlan")
        return cls(
            read_views=tuple_from_json(raw, "read_views", ResourceReadView),
            write_locks=tuple_from_json(raw, "write_locks", ResourceWriteLock),
            parallel_groups=_str_group_tuple(
                field_value(raw, "parallel_groups"), "parallel_groups"
            ),
            serial_groups=_str_group_tuple(field_value(raw, "serial_groups"), "serial_groups"),
            parallelism_limit=as_int(field_value(raw, "parallelism_limit"), "parallelism_limit"),
            version_checks=tuple_from_json(raw, "version_checks", VersionExpectation),
            deferred_writes=tuple_from_json(raw, "deferred_writes", DeferredResourceOp),
            conflict_entries=as_str_tuple(
                field_value(raw, "conflict_entries"), "conflict_entries"
            ),
        )


@dataclass(frozen=True)
class TaskBatch:
    batch_id: str
    tick_id: str | None
    tasks: tuple[Task, ...]
    resource_plan: WorkResourcePlan | None = None

    @classmethod
    def one(cls, batch_id: str, task: Task) -> Self:
        return cls(batch_id=batch_id, tick_id=None, tasks=(task,), resource_plan=None)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskBatch")
        resource_plan = field_value(raw, "resource_plan")
        return cls(
            batch_id=as_str(field_value(raw, "batch_id"), "batch_id"),
            tick_id=optional_str(field_value(raw, "tick_id"), "tick_id"),
            tasks=tuple_from_json(raw, "tasks", Task),
            resource_plan=None
            if resource_plan is None
            else WorkResourcePlan.from_json_dict(as_mapping(resource_plan, "resource_plan")),
        )


@dataclass(frozen=True)
class WorkSet:
    tick_id: str
    batch_key: str
    entries: tuple[BatchEntry, ...]
    resource_requirements: tuple[ResourceRequirement, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "WorkSet")
        return cls(
            tick_id=as_str(field_value(raw, "tick_id"), "tick_id"),
            batch_key=as_str(field_value(raw, "batch_key"), "batch_key"),
            entries=tuple_from_json(raw, "entries", BatchEntry),
            resource_requirements=tuple_from_json(
                raw, "resource_requirements", ResourceRequirement
            ),
        )


@dataclass(frozen=True)
class WorkBatch:
    batch_id: str
    tick_id: str
    batch_key: str
    entries: tuple[BatchEntry, ...]
    payload: BatchPayload
    resource_plan: WorkResourcePlan
    task_leases: tuple[TaskLease, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "WorkBatch")
        return cls(
            batch_id=as_str(field_value(raw, "batch_id"), "batch_id"),
            tick_id=as_str(field_value(raw, "tick_id"), "tick_id"),
            batch_key=as_str(field_value(raw, "batch_key"), "batch_key"),
            entries=tuple_from_json(raw, "entries", BatchEntry),
            payload=BatchPayload.from_json_dict(as_mapping(field_value(raw, "payload"), "payload")),
            resource_plan=WorkResourcePlan.from_json_dict(
                as_mapping(field_value(raw, "resource_plan"), "resource_plan")
            ),
            task_leases=tuple_from_json(raw, "task_leases", TaskLease),
        )

    def row_payload_tasks(self) -> tuple[Task, ...] | RuntimeError:
        return self.payload.try_row_tasks()


@dataclass(frozen=True)
class EntryCompletion:
    entry_id: str
    task_id: str
    result: RunnerResult | None = None
    error: RuntimeError | None = None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        from mutsuki_runner_kit.contracts.runner import RunnerResult

        raw = as_mapping(data, "EntryCompletion")
        result = field_value(raw, "result")
        error = field_value(raw, "error")
        return cls(
            entry_id=as_str(field_value(raw, "entry_id"), "entry_id"),
            task_id=as_str(field_value(raw, "task_id"), "task_id"),
            result=None
            if result is None
            else RunnerResult.from_json_dict(as_mapping(result, "result")),
            error=None
            if error is None
            else RuntimeError.from_json_dict(as_mapping(error, "error")),
        )


@dataclass(frozen=True)
class CompletionBatch:
    batch_id: str
    tick_id: str
    results: tuple[EntryCompletion, ...]
    metadata: tuple[tuple[str, ScalarValue], ...] = ()

    @classmethod
    def from_results(cls, batch: WorkBatch, results: Sequence[EntryCompletion]) -> Self:
        return cls(
            batch_id=batch.batch_id,
            tick_id=batch.tick_id,
            results=tuple(results),
            metadata=(),
        )

    @classmethod
    def from_error(cls, batch: WorkBatch, error: RuntimeError) -> Self:
        return cls.from_results(
            batch,
            [
                EntryCompletion(
                    entry_id=entry.entry_id,
                    task_id=entry.task_id,
                    result=None,
                    error=error,
                )
                for entry in batch.entries
            ],
        )

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "CompletionBatch")
        return cls(
            batch_id=as_str(field_value(raw, "batch_id"), "batch_id"),
            tick_id=as_str(field_value(raw, "tick_id"), "tick_id"),
            results=tuple_from_json(raw, "results", EntryCompletion),
            metadata=_metadata_pairs(field_value(raw, "metadata"), "metadata"),
        )

    def to_json_value(self) -> JsonDict:
        return {
            "batch_id": self.batch_id,
            "tick_id": self.tick_id,
            "results": [to_json_value(item) for item in self.results],
            "metadata": [[key, value] for key, value in self.metadata],
        }


def _int_tuple(value: object, field_name: str) -> tuple[int, ...]:
    return tuple(as_int(item, field_name) for item in sequence(value, field_name))


def _byte_tuple(value: object, field_name: str) -> tuple[int, ...]:
    bytes_values = _int_tuple(value, field_name)
    for item in bytes_values:
        if item < 0 or item > 255:
            raise TypeError(f"{field_name} expects byte values")
    return bytes_values


def _str_group_tuple(value: object, field_name: str) -> tuple[tuple[str, ...], ...]:
    return tuple(as_str_tuple(group, field_name) for group in sequence(value, field_name))


def _metadata_pairs(
    value: object, field_name: str
) -> tuple[tuple[str, ScalarValue], ...]:
    pairs: list[tuple[str, ScalarValue]] = []
    for item in sequence(value, field_name):
        if not isinstance(item, Sequence) or isinstance(item, str | bytes | bytearray):
            raise TypeError(f"{field_name} expects sequence of pairs")
        if len(item) != 2:
            raise TypeError(f"{field_name} expects [key, value] pairs")
        pairs.append((as_str(item[0], field_name), as_scalar(item[1], field_name)))
    return tuple(pairs)
