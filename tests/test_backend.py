from __future__ import annotations

from dataclasses import replace

import pytest

from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError
from mutsuki_runner_kit.testing.batches import multi_entry_batch, runner_context, single_test_batch
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor


class CaptureContextRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        super().__init__(descriptor)
        self.contexts: list[RunnerContext] = []

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        self.contexts.append(ctx)
        return await super().run_one(ctx, task)


@pytest.mark.asyncio
async def test_python_runner_backend_runs_registered_runner_batch() -> None:
    backend = PythonRunnerBackend()
    runner = EchoRunner(echo_descriptor())
    backend.register_runner(runner)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")

    completion = await backend.run_batch_runner(
        "echo.runner",
        runner_context(),
        single_test_batch(task),
    )

    assert completion.batch_id == "batch-test"
    assert completion.results[0].task_id == "task-1"
    assert completion.results[0].result is not None
    assert completion.results[0].result.task_id == "task-1"
    assert backend.descriptors()[0].runner_id == "echo.runner"


@pytest.mark.asyncio
async def test_python_runner_backend_cancel_and_dispose_are_management_channel() -> None:
    backend = PythonRunnerBackend()
    runner = EchoRunner(echo_descriptor())
    backend.register_runner(runner)

    await backend.cancel_runner("echo.runner", "inv-1")
    await backend.dispose_runner("echo.runner")

    assert runner.cancelled == ["inv-1"]
    assert runner.disposed is True


@pytest.mark.asyncio
async def test_python_runner_backend_propagates_prior_cancel_into_next_batch_context() -> None:
    backend = PythonRunnerBackend()
    runner = CaptureContextRunner(echo_descriptor())
    backend.register_runner(runner)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")

    await backend.cancel_runner("echo.runner", "task-1")
    completion = await backend.run_batch_runner(
        "echo.runner",
        runner_context(deadline_tick=3, current_step=2),
        single_test_batch(task),
    )

    assert completion.results[0].task_id == "task-1"
    assert runner.contexts[0].cancel_requested is True
    assert runner.contexts[0].deadline_tick == 3


@pytest.mark.asyncio
async def test_python_runner_backend_rejects_task_lease_mismatch() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(EchoRunner(echo_descriptor()))
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-task")

    with pytest.raises(RunnerInvokeError) as exc_info:
        await backend.run_batch_runner(
            "echo.runner",
            runner_context(lease_ids=("task-lease-ctx",)),
            single_test_batch(task, lease_id="task-lease-task"),
        )

    assert exc_info.value.error.code == "task.claim_conflict"


@pytest.mark.asyncio
async def test_python_runner_backend_returns_independent_entry_completions() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(EchoRunner(echo_descriptor()))
    tasks = (
        Task.new("task-1", "raw.input"),
        Task.new("task-2", "raw.input"),
    )
    lease_ids = ("lease-1", "lease-2")

    completion = await backend.run_batch_runner(
        "echo.runner",
        runner_context(lease_ids=lease_ids, batch_id="batch-multi"),
        multi_entry_batch(tasks, lease_ids=lease_ids),
    )

    assert [item.entry_id for item in completion.results] == ["task-1", "task-2"]
    assert all(item.result is not None and item.error is None for item in completion.results)


@pytest.mark.asyncio
async def test_pending_cancel_is_scoped_by_runner_and_bounded() -> None:
    backend = PythonRunnerBackend(max_pending_cancels=1)
    runner_a = CaptureContextRunner(replace(echo_descriptor(), runner_id="runner-a"))
    runner_b = CaptureContextRunner(replace(echo_descriptor(), runner_id="runner-b"))
    backend.register_runner(runner_a)
    backend.register_runner(runner_b)

    await backend.cancel_runner("runner-a", "shared-invocation")
    with pytest.raises(RunnerInvokeError) as exc_info:
        await backend.cancel_runner("runner-b", "other-invocation")

    assert exc_info.value.error.code == "capability.exhausted"
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    await backend.run_batch_runner(
        "runner-b",
        replace(runner_context(), invocation_id="shared-invocation"),
        single_test_batch(task),
    )
    assert runner_b.contexts[0].cancel_requested is False
