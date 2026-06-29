from __future__ import annotations

from dataclasses import replace

import pytest

from mutsuki_runtime_python.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runtime_python.contracts.task import Task
from mutsuki_runtime_python.runners.host import PythonRunnerHost
from mutsuki_runtime_python.runners.protocol import RunnerInvokeError
from mutsuki_runtime_python.testing.runners import EchoRunner, echo_descriptor


class CaptureContextRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        super().__init__(descriptor)
        self.contexts: list[RunnerContext] = []

    async def step(
        self, ctx: RunnerContext, tasks: tuple[Task, ...]
    ) -> tuple[RunnerResult, ...]:
        self.contexts.append(ctx)
        return await super().step(ctx, tasks)


@pytest.mark.asyncio
async def test_python_runner_host_steps_registered_runner() -> None:
    host = PythonRunnerHost()
    runner = EchoRunner(echo_descriptor())
    host.register_runner(runner)

    results = await host.step_runner(
        "echo.runner",
        RunnerContext(
            registry_generation=1,
            current_step=1,
            executor_id="executor:test",
            task_lease_id="task-lease-test",
        ),
        (replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test"),),
    )

    assert results[0].task_id == "task-1"
    assert host.descriptors()[0].runner_id == "echo.runner"


@pytest.mark.asyncio
async def test_python_runner_host_cancel_and_dispose_are_management_channel() -> None:
    host = PythonRunnerHost()
    runner = EchoRunner(echo_descriptor())
    host.register_runner(runner)

    await host.cancel_runner("echo.runner", "inv-1")
    await host.dispose_runner("echo.runner")

    assert runner.cancelled == ["inv-1"]
    assert runner.disposed is True


@pytest.mark.asyncio
async def test_python_runner_host_propagates_prior_cancel_into_next_step_context() -> None:
    host = PythonRunnerHost()
    runner = CaptureContextRunner(echo_descriptor())
    host.register_runner(runner)

    await host.cancel_runner("echo.runner", "task-1")
    results = await host.step_runner(
        "echo.runner",
        RunnerContext(
            registry_generation=1,
            current_step=2,
            executor_id="executor:test",
            task_lease_id="task-lease-test",
            invocation_id="task-1",
            cancel_token="task-1",
            deadline_tick=3,
        ),
        (replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test"),),
    )

    assert results[0].task_id == "task-1"
    assert runner.contexts[0].cancel_requested is True
    assert runner.contexts[0].deadline_tick == 3


@pytest.mark.asyncio
async def test_python_runner_host_rejects_task_lease_mismatch() -> None:
    host = PythonRunnerHost()
    host.register_runner(EchoRunner(echo_descriptor()))

    with pytest.raises(RunnerInvokeError) as exc_info:
        await host.step_runner(
            "echo.runner",
            RunnerContext(
                registry_generation=1,
                current_step=1,
                executor_id="executor:test",
                task_lease_id="task-lease-ctx",
            ),
            (replace(Task.new("task-1", "raw.input"), lease_id="task-lease-task"),),
        )

    assert exc_info.value.error.code == "task.claim_conflict"
