from __future__ import annotations

from mutsuki_runner_kit.contracts.batch import (
    BatchEntry,
    BatchPayload,
    BinaryPackedPayload,
    ColumnarPayload,
    ColumnPayload,
    CompletionBatch,
    DispatchLane,
    EntryCompletion,
    OrderingRequirement,
    PayloadLayout,
    ResourceAccessMode,
    ResourceBackedPayload,
    ResourceReadView,
    ResourceRequirement,
    ResourceSlice,
    ResourceWriteLock,
    TaskBatch,
    WorkBatch,
    WorkResourcePlan,
    WorkSet,
)
from mutsuki_runner_kit.contracts.resource import (
    ResourceAccess,
    ResourceId,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ResourceSemantic,
)
from mutsuki_runner_kit.contracts.runner import RunnerResult
from mutsuki_runner_kit.contracts.state import VersionExpectation
from mutsuki_runner_kit.contracts.task import Task, TaskLease
from mutsuki_runner_kit.testing.assertions import assert_json_roundtrip


def _resource_ref(ref_id: str = "resource:1") -> ResourceRef:
    return ResourceRef(
        ref_id=ref_id,
        resource_id=ResourceId(kind_id="bytes", slot_id=ref_id, generation=1, version=1),
        semantic=ResourceSemantic.FROZEN_VALUE,
        provider_id="python.resource",
        resource_kind="bytes",
        schema="bytes.v1",
        version=1,
        generation=1,
        access=ResourceAccess.inline(),
        size_hint=None,
        content_hash=None,
        lifetime=ResourceLifetime.PERSISTENT,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )


def test_work_batch_and_completion_batch_roundtrip() -> None:
    entry = BatchEntry(
        entry_id="entry-1",
        task_id="task-1",
        trace_id="trace-1",
        parent_id=None,
        payload_index=0,
        resource_requirement_indices=(0,),
        cancel_index=0,
        deadline_tick=20,
        priority=1,
        lane=DispatchLane.INTERACTIVE,
        ordering=OrderingRequirement.preserve_submit_order(),
    )
    batch = WorkBatch(
        batch_id="batch-1",
        tick_id="tick-10",
        batch_key="runner-a",
        entries=(entry,),
        payload=BatchPayload.row([{"input": 1}]),
        resource_plan=WorkResourcePlan(
            read_views=(ResourceReadView(ref_id="resource:1", requirement_indices=(0,)),),
            write_locks=(ResourceWriteLock(ref_id="resource:2", requirement_indices=(1,)),),
            serial_groups=(("entry-1",),),
            version_checks=(VersionExpectation(ref_id="resource:1", expected_version=1),),
        ),
        task_leases=(
            TaskLease(
                lease_id="lease-1",
                task_id="task-1",
                runner_id="runner-a",
                executor_id="executor-a",
                registry_generation=3,
                acquired_at_step=10,
                expires_at_step=11,
            ),
        ),
    )
    assert_json_roundtrip(WorkBatch, batch)
    assert_json_roundtrip(
        CompletionBatch,
        CompletionBatch(
            batch_id="batch-1",
            tick_id="tick-10",
            results=(
                EntryCompletion(
                    entry_id="entry-1",
                    task_id="task-1",
                    result=RunnerResult.completed("task-1"),
                ),
            ),
            metadata=(("payload_layout", "row"),),
        ),
    )


def test_task_batch_and_work_set_roundtrip() -> None:
    task = Task.new("task-submit-1", "raw.input", {"input": 1})
    assert_json_roundtrip(
        TaskBatch,
        TaskBatch(
            batch_id="submit-batch-1",
            tick_id="tick-10",
            tasks=(task,),
            resource_plan=WorkResourcePlan.empty(),
        ),
    )
    assert TaskBatch.one("submit-batch-1", task).tasks == (task,)
    assert_json_roundtrip(
        WorkSet,
        WorkSet(
            tick_id="tick-10",
            batch_key="runner-a",
            entries=(
                BatchEntry(
                    entry_id="entry-1",
                    task_id="task-1",
                    trace_id=None,
                    parent_id=None,
                    payload_index=0,
                    resource_requirement_indices=(),
                    cancel_index=None,
                    deadline_tick=None,
                    priority=0,
                    lane=DispatchLane.NORMAL,
                    ordering=OrderingRequirement.none(),
                ),
            ),
            resource_requirements=(
                ResourceRequirement(
                    ref_id="resource:1",
                    mode=ResourceAccessMode.READ,
                    expected_version=1,
                ),
            ),
        ),
    )


def test_batch_payload_layouts_and_row_task_decode() -> None:
    row = BatchPayload.from_tasks((Task.new("payload-task-1", "raw.input", {"input": 1}),))
    decoded = row.try_row_tasks()
    assert row.layout == PayloadLayout.ROW
    assert isinstance(decoded, tuple)
    assert decoded[0].task_id == "payload-task-1"
    assert_json_roundtrip(BatchPayload, row)
    assert_json_roundtrip(
        BatchPayload,
        BatchPayload(
            layout=PayloadLayout.COLUMNAR,
            payload=ColumnarPayload(
                columns=(ColumnPayload(name="input", values=(1,)),),
                row_count=1,
            ),
        ),
    )
    assert_json_roundtrip(
        BatchPayload,
        BatchPayload(
            layout=PayloadLayout.BINARY_PACKED,
            payload=BinaryPackedPayload(encoding="raw", bytes=(1, 2, 3), row_count=1),
        ),
    )
    assert_json_roundtrip(
        BatchPayload,
        BatchPayload(
            layout=PayloadLayout.RESOURCE_BACKED,
            payload=ResourceBackedPayload(
                slices=(ResourceSlice(resource=_resource_ref(), offset=0, length=3),)
            ),
        ),
    )
    assert_json_roundtrip(OrderingRequirement, OrderingRequirement.same_resource_order("r:1"))
    assert_json_roundtrip(OrderingRequirement, OrderingRequirement.strict_sequence("seq-1"))
