from __future__ import annotations

from collections.abc import Generator
from dataclasses import replace

import pytest

from mutsuki_runner_kit.contracts.runner import (
    ExecutionClass,
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
    RunnerStatus,
)
from mutsuki_runner_kit.contracts.task import CancelPolicy, Task, TaskOutcome
from mutsuki_runner_kit.runners.async_adapter import AsyncRunnerAdapter, AsyncRunnerContext
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError
from mutsuki_runner_kit.testing.batches import runner_context


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


def async_runner_context(**kwargs: object):
    defaults = {
        "lease_ids": ("lease:test",),
        "invocation_id": "parent-1",
        "batch_id": "parent-1",
        "tick_id": "tick-1",
    }
    defaults.update(kwargs)
    return runner_context(**defaults)  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_async_runner_context_exposes_deadline_and_cancel_fields() -> None:
    client = ManualClient()
    observed: dict[str, object] = {}

    async def run(ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        observed["invocation_id"] = ctx.invocation_id
        observed["cancel_token"] = ctx.cancel_token
        observed["deadline_tick"] = ctx.deadline_tick
        observed["cancel_requested"] = ctx.cancel_requested
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run)
    ctx = replace(async_runner_context(), deadline_tick=3, cancel_requested=True)

    result = await adapter.run_one(ctx, Task.new("parent-1", "parent.work"))

    assert result.status == RunnerStatus.COMPLETED
    assert observed == {
        "invocation_id": "parent-1",
        "cancel_token": "parent-1",
        "deadline_tick": 3,
        "cancel_requested": True,
    }


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

    first = await adapter.run_one(async_runner_context(), task)

    assert first.status == RunnerStatus.WAITING
    assert first.tasks[0].task_id == "parent-1:call:1"
    assert first.tasks[0].protocol_id == "child.work"
    assert first.tasks[0].trace_id == "trace-1"
    assert first.tasks[0].correlation_id == "corr-1"
    assert first.task_await is not None
    assert first.task_await.cancel_policy == CancelPolicy.CASCADE
    assert first.task_await.child.trace_id == "trace-1"
    assert first.task_await.child.correlation_id == "corr-1"

    client.outcomes["parent-1:call:1"] = TaskOutcome.completed("parent-1:call:1")
    second = await adapter.run_one(async_runner_context(), task)

    assert second.status == RunnerStatus.COMPLETED


@pytest.mark.asyncio
async def test_async_runner_adapter_cancel_removes_invocation_by_invocation_id() -> None:
    client = ManualClient()

    async def run(ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        await ctx.call_raw("child.work", {"from": task.task_id})
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run)
    task = Task.new("parent-1", "parent.work")

    first = await adapter.run_one(
        replace(async_runner_context(), invocation_id="invocation:one"),
        task,
    )

    assert first.status == RunnerStatus.WAITING
    client.outcomes["parent-1:call:1"] = TaskOutcome.completed("parent-1:call:1")

    await adapter.cancel("invocation:one")
    second = await adapter.run_one(
        replace(async_runner_context(), invocation_id="invocation:two"),
        task,
    )

    assert second.status == RunnerStatus.WAITING
    assert second.tasks[0].task_id == "parent-1:call:1"


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

    first = await adapter.run_one(
        async_runner_context(), Task.new("parent-1", "parent.work")
    )

    assert first.status == RunnerStatus.WAITING
    assert first.tasks[0].target_binding_id == "binding:child"
    assert first.tasks[0].runner_hint == "child.runner"
    assert first.task_await is not None
    assert first.task_await.child.target_binding_id == "binding:child"


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

    first = await adapter.run_one(
        async_runner_context(), Task.new("parent-1", "parent.work")
    )

    assert first.task_await is not None
    assert first.task_await.cancel_policy == CancelPolicy.SHIELD
    assert first.task_await.child.cancel_policy == CancelPolicy.SHIELD


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
        await adapter.run_one(
            async_runner_context(), Task.new("parent-1", "parent.work")
        )

    assert exc_info.value.error.code == "task.self_call_blocked"


@pytest.mark.asyncio
async def test_async_runner_adapter_rejects_non_mutsuki_awaitable() -> None:
    client = ManualClient()

    async def run(_ctx: AsyncRunnerContext, task: Task) -> RunnerResult:
        await _plain_awaitable()
        return RunnerResult.completed(task.task_id)

    adapter = AsyncRunnerAdapter(async_descriptor(), client, run)

    with pytest.raises(RunnerInvokeError) as exc_info:
        await adapter.run_one(
            async_runner_context(), Task.new("parent-1", "parent.work")
        )

    assert exc_info.value.error.code == "runner.awaitable_unsupported"


async def _plain_awaitable() -> None:
    await _PlainAwaitable()


class _PlainAwaitable:
    def __await__(self) -> Generator[str]:
        yield "plain"
        return None
