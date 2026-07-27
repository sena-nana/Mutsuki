from __future__ import annotations

from dataclasses import replace
from typing import Protocol

from mutsuki_runner_kit.contracts.batch import (
    BatchEntry,
    BatchPayload,
    CompletionBatch,
    DispatchLane,
    EntryCompletion,
    OrderingRequirement,
    WorkBatch,
    WorkResourcePlan,
)
from mutsuki_runner_kit.contracts.errors import ERR_TASK_CLAIM_CONFLICT, RuntimeError
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task, TaskLease
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError


class ScalarPythonRunner(Protocol):
    @property
    def descriptor(self) -> RunnerDescriptor: ...

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult: ...

    async def cancel(self, invocation_id: str) -> None: ...

    async def dispose(self) -> None: ...


class ScalarBatchAdapter:
    """Wrap a scalar `run_one` implementation as the wire `run_batch` ABI."""

    def __init__(self, runner: ScalarPythonRunner) -> None:
        self._runner = runner

    @property
    def descriptor(self) -> RunnerDescriptor:
        return self._runner.descriptor

    async def run_batch(self, ctx: RunnerContext, batch: WorkBatch) -> CompletionBatch:
        decoded = batch.row_payload_tasks()
        if isinstance(decoded, RuntimeError):
            return CompletionBatch.from_error(batch, decoded)

        results: list[EntryCompletion] = []
        for entry in batch.entries:
            task = next((item for item in decoded if item.task_id == entry.task_id), None)
            if task is None:
                results.append(
                    EntryCompletion(
                        entry_id=entry.entry_id,
                        task_id=entry.task_id,
                        error=RuntimeError(
                            code=ERR_TASK_CLAIM_CONFLICT,
                            source="python_scalar_batch_adapter",
                            route=f"batch.entry.{entry.entry_id}",
                        ),
                    )
                )
                continue
            try:
                result = await self._runner.run_one(ctx, task)
            except RunnerInvokeError as exc:
                results.append(
                    EntryCompletion(
                        entry_id=entry.entry_id,
                        task_id=entry.task_id,
                        error=exc.error,
                    )
                )
                continue
            results.append(
                EntryCompletion(entry_id=entry.entry_id, task_id=entry.task_id, result=result)
            )
        return CompletionBatch.from_results(batch, results)

    async def cancel(self, invocation_id: str) -> None:
        await self._runner.cancel(invocation_id)

    async def dispose(self) -> None:
        await self._runner.dispose()


def single_entry_batch(ctx: RunnerContext, task: Task, *, runner_id: str) -> WorkBatch:
    lease_id = ctx.task_lease_ids[0] if ctx.task_lease_ids else f"lease:{task.task_id}"
    leased_task = replace(task, lease_id=lease_id)
    return WorkBatch(
        batch_id=ctx.batch_id,
        tick_id=ctx.tick_id,
        batch_key=runner_id,
        entries=(
            BatchEntry(
                entry_id=leased_task.task_id,
                task_id=leased_task.task_id,
                trace_id=leased_task.trace_id,
                parent_id=None,
                payload_index=0,
                resource_requirement_indices=(),
                cancel_index=0,
                deadline_tick=ctx.deadline_tick,
                priority=leased_task.priority,
                lane=DispatchLane.NORMAL,
                ordering=OrderingRequirement.none(),
            ),
        ),
        payload=BatchPayload.from_tasks((leased_task,)),
        resource_plan=WorkResourcePlan.empty(),
        task_leases=(
            TaskLease(
                lease_id=lease_id,
                task_id=leased_task.task_id,
                runner_id=runner_id,
                executor_id=ctx.executor_id,
                registry_generation=ctx.registry_generation,
                acquired_at_step=ctx.current_step,
                expires_at_step=None,
            ),
        ),
    )
