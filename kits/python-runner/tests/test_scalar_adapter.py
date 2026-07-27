from __future__ import annotations

from dataclasses import replace

import pytest

from mutsuki_runner_kit.contracts.batch import CompletionBatch
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.scalar import ScalarBatchAdapter, single_entry_batch
from mutsuki_runner_kit.testing.batches import multi_entry_batch, runner_context
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor


@pytest.mark.asyncio
async def test_scalar_adapter_wraps_run_one_as_run_batch() -> None:
    runner = EchoRunner(echo_descriptor())
    ctx = runner_context()
    completion = await ScalarBatchAdapter(runner).run_batch(
        ctx,
        single_entry_batch(ctx, Task.new("task-1", "raw.input"), runner_id="echo.runner"),
    )
    assert isinstance(completion, CompletionBatch)
    assert completion.results[0].result == RunnerResult.completed("task-1")


@pytest.mark.asyncio
async def test_scalar_adapter_returns_independent_entry_completions() -> None:
    class CountingRunner(EchoRunner):
        def __init__(self) -> None:
            super().__init__(echo_descriptor())
            self.seen: list[str] = []

        async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
            self.seen.append(task.task_id)
            return RunnerResult.completed(task.task_id)

    runner = CountingRunner()
    lease_ids = ("lease-1", "lease-2")
    completion = await ScalarBatchAdapter(runner).run_batch(
        runner_context(lease_ids=lease_ids, batch_id="batch-multi"),
        multi_entry_batch(
            (Task.new("task-1", "raw.input"), Task.new("task-2", "raw.input")),
            lease_ids=lease_ids,
        ),
    )
    assert runner.seen == ["task-1", "task-2"]
    assert [item.entry_id for item in completion.results] == ["task-1", "task-2"]


@pytest.mark.asyncio
async def test_single_entry_batch_uses_ctx_lease_ids() -> None:
    batch = single_entry_batch(
        runner_context(lease_ids=("lease-from-ctx",)),
        replace(Task.new("task-1", "raw.input"), lease_id=None),
        runner_id="echo.runner",
    )
    decoded = batch.row_payload_tasks()
    assert batch.task_leases[0].lease_id == "lease-from-ctx"
    assert not isinstance(decoded, RuntimeError)
    assert decoded[0].lease_id == "lease-from-ctx"
