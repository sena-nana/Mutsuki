from __future__ import annotations

from mutsuki_runner_kit.contracts.batch import CompletionBatch, WorkBatch
from mutsuki_runner_kit.contracts.runner import (
    ExecutionClass,
    RunnerContext,
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
)
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.scalar import ScalarBatchAdapter


class EchoRunner:
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        self._descriptor = descriptor
        self.cancelled: list[str] = []
        self.disposed = False
        self._adapter = ScalarBatchAdapter(self)

    @property
    def descriptor(self) -> RunnerDescriptor:
        return self._descriptor

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        return RunnerResult.completed(task.task_id)

    async def run_batch(self, ctx: RunnerContext, batch: WorkBatch) -> CompletionBatch:
        return await self._adapter.run_batch(ctx, batch)

    async def cancel(self, invocation_id: str) -> None:
        self.cancelled.append(invocation_id)

    async def dispose(self) -> None:
        self.disposed = True


def echo_descriptor() -> RunnerDescriptor:
    return RunnerDescriptor(
        runner_id="echo.runner",
        plugin_id="plugin.echo",
        plugin_generation=1,
        accepted_protocol_ids=("raw.input",),
        purity=RunnerPurity.PURE,
        execution_class=ExecutionClass.CPU,
        contract_surfaces=("runner:echo.runner",),
    )
