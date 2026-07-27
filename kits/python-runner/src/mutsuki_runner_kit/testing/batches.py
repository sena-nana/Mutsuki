from __future__ import annotations

from dataclasses import replace

from mutsuki_runner_kit.contracts.batch import (
    BatchEntry,
    BatchPayload,
    DispatchLane,
    OrderingRequirement,
    WorkBatch,
    WorkResourcePlan,
)
from mutsuki_runner_kit.contracts.runner import RunnerContext
from mutsuki_runner_kit.contracts.task import Task, TaskLease


def runner_context(
    *,
    lease_ids: tuple[str, ...] = ("task-lease-test",),
    invocation_id: str = "task-1",
    batch_id: str = "batch-test",
    tick_id: str = "tick-1",
    current_step: int = 1,
    deadline_tick: int | None = None,
    cancel_requested: bool = False,
) -> RunnerContext:
    return RunnerContext(
        registry_generation=1,
        current_step=current_step,
        tick_id=tick_id,
        batch_id=batch_id,
        executor_id="executor:test",
        task_lease_ids=lease_ids,
        entry_count=len(lease_ids),
        invocation_id=invocation_id,
        cancel_token=invocation_id,
        deadline_tick=deadline_tick,
        cancel_requested=cancel_requested,
    )


def single_test_batch(
    task: Task,
    *,
    lease_id: str = "task-lease-test",
    batch_id: str = "batch-test",
    tick_id: str = "tick-1",
    runner_id: str = "echo.runner",
) -> WorkBatch:
    leased = replace(task, lease_id=lease_id)
    lease = TaskLease(
        lease_id=lease_id,
        task_id=leased.task_id,
        runner_id=runner_id,
        executor_id="executor:test",
        registry_generation=1,
        acquired_at_step=1,
        expires_at_step=None,
    )
    return WorkBatch(
        batch_id=batch_id,
        tick_id=tick_id,
        batch_key=runner_id,
        entries=(
            BatchEntry(
                entry_id=leased.task_id,
                task_id=leased.task_id,
                trace_id=leased.trace_id,
                parent_id=None,
                payload_index=0,
                resource_requirement_indices=(),
                cancel_index=0,
                deadline_tick=None,
                priority=leased.priority,
                lane=DispatchLane.NORMAL,
                ordering=OrderingRequirement.none(),
            ),
        ),
        payload=BatchPayload.from_tasks((leased,)),
        resource_plan=WorkResourcePlan.empty(),
        task_leases=(lease,),
    )


def multi_entry_batch(
    tasks: tuple[Task, ...],
    *,
    lease_ids: tuple[str, ...],
    batch_id: str = "batch-multi",
    tick_id: str = "tick-1",
    runner_id: str = "echo.runner",
) -> WorkBatch:
    if len(tasks) != len(lease_ids):
        raise ValueError("tasks and lease_ids must align")
    leased_tasks = tuple(
        replace(task, lease_id=lease_id) for task, lease_id in zip(tasks, lease_ids, strict=True)
    )
    entries = tuple(
        BatchEntry(
            entry_id=task.task_id,
            task_id=task.task_id,
            trace_id=task.trace_id,
            parent_id=None,
            payload_index=index,
            resource_requirement_indices=(),
            cancel_index=index,
            deadline_tick=None,
            priority=task.priority,
            lane=DispatchLane.NORMAL,
            ordering=OrderingRequirement.none(),
        )
        for index, task in enumerate(leased_tasks)
    )
    leases = tuple(
        TaskLease(
            lease_id=lease_id,
            task_id=task.task_id,
            runner_id=runner_id,
            executor_id="executor:test",
            registry_generation=1,
            acquired_at_step=1,
            expires_at_step=None,
        )
        for task, lease_id in zip(leased_tasks, lease_ids, strict=True)
    )
    return WorkBatch(
        batch_id=batch_id,
        tick_id=tick_id,
        batch_key=runner_id,
        entries=entries,
        payload=BatchPayload.from_tasks(leased_tasks),
        resource_plan=WorkResourcePlan.empty(),
        task_leases=leases,
    )
