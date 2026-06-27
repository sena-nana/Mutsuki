from __future__ import annotations

from collections.abc import Generator
from dataclasses import replace

import pytest

from mutsuki_runtime_python.contracts.runner import (
    ExecutionClass,
    RunnerContext,
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
    RunnerStatus,
)
from mutsuki_runtime_python.contracts.task import CancelPolicy, Task, TaskOutcome
from mutsuki_runtime_python.runners.async_adapter import AsyncRunnerAdapter, AsyncRunnerContext
from mutsuki_runtime_python.runners.protocol import RunnerInvokeError


class ManualClient:
    def __init__(self) -> None:
        self.outcomes: dict[str, TaskOutcome] = {}

    def task_outcome(self, task_id: str) -> TaskOutcome | None:
        return self.outcomes.get(task_id)


def async_descriptor() -> RunnerDescriptor:
    return RunnerDescriptor(
        runner_id="async.runner",
        plugin_id="plugin-a",
        plugin_generation=1,
        accepted_protocol_ids=("parent.work",),
        purity=RunnerPurity.PURE,
        execution_class=ExecutionClass.CPU,
        contract_surfaces=("runner:async.runner",),
    )


def runner_context() -> RunnerContext:
    return RunnerContext(
        registry_generation=1,
        current_step=1,
        executor_id="executor:test",
        task_lease_id="lease:test",
    )


@pytest.mark.asyncio
async def test_async_runner_adapter_suspends_and_resumes_call() -> None:
    client = ManualClient()

    async def run(ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        outcome = await ctx.call_raw("child.work", {"from": task.task_id})
        assert outcome.status.value == "completed"
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run)
    task = replace(
        Task.new("parent-1", "parent.work"),
        trace_id="trace-1",
        correlation_id="corr-1",
    )

    first = await adapter.step(runner_context(), (task,))

    assert first[0].status == RunnerStatus.WAITING
    assert first[0].tasks[0].task_id == "parent-1:call:1"
    assert first[0].tasks[0].protocol_id == "child.work"
    assert first[0].tasks[0].trace_id == "trace-1"
    assert first[0].tasks[0].correlation_id == "corr-1"
    assert first[0].task_await is not None
    assert first[0].task_await.cancel_policy == CancelPolicy.CASCADE

    client.outcomes["parent-1:call:1"] = TaskOutcome.completed("parent-1:call:1")
    second = await adapter.step(runner_context(), (task,))

    assert second[0].status == RunnerStatus.COMPLETED


@pytest.mark.asyncio
async def test_async_runner_adapter_emits_targeted_child_task_descriptor() -> None:
    client = ManualClient()

    async def run(ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        await ctx.call_targeted_raw(
            "binding:child",
            "child.work",
            "child.runner",
            {"from": task.task_id},
        )
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run)

    first = await adapter.step(runner_context(), (Task.new("parent-1", "parent.work"),))

    assert first[0].status == RunnerStatus.WAITING
    assert first[0].tasks[0].target_binding_id == "binding:child"
    assert first[0].tasks[0].runner_hint == "child.runner"
    assert first[0].task_await is not None
    assert first[0].task_await.child.target_binding_id == "binding:child"


@pytest.mark.asyncio
async def test_async_runner_adapter_emits_explicit_cancel_policy_descriptor() -> None:
    client = ManualClient()

    async def run(ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        await ctx.call_with_cancel_policy(
            "child.work",
            {"from": task.task_id},
            CancelPolicy.SHIELD,
        )
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run)

    first = await adapter.step(runner_context(), (Task.new("parent-1", "parent.work"),))

    assert first[0].task_await is not None
    assert first[0].task_await.cancel_policy == CancelPolicy.SHIELD
    assert first[0].task_await.child.cancel_policy == CancelPolicy.SHIELD


@pytest.mark.asyncio
async def test_async_runner_adapter_rejects_self_call_when_policy_disallows_it() -> None:
    client = ManualClient()

    async def run(ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        await ctx.call_targeted_raw(
            "binding:self",
            "parent.work",
            "async.runner",
            {"from": task.task_id},
        )
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run, allow_self_call=False)

    with pytest.raises(RunnerInvokeError) as exc_info:
        await adapter.step(runner_context(), (Task.new("parent-1", "parent.work"),))

    assert exc_info.value.error.code == "task.self_call_blocked"


@pytest.mark.asyncio
async def test_async_runner_adapter_rejects_non_mutsuki_awaitable() -> None:
    client = ManualClient()

    async def run(_ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        await _plain_awaitable()
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run)

    with pytest.raises(RunnerInvokeError) as exc_info:
        await adapter.step(runner_context(), (Task.new("parent-1", "parent.work"),))

    assert exc_info.value.error.code == "runner.awaitable_unsupported"


async def _plain_awaitable() -> None:
    await _PlainAwaitable()


class _PlainAwaitable:
    def __await__(self) -> Generator[str]:
        yield "plain"
        return None
