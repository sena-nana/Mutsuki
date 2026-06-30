from __future__ import annotations

from dataclasses import replace

from mutsuki_runner_kit.contracts.errors import (
    ERR_RUNNER_NOT_FOUND,
    ERR_TASK_CLAIM_CONFLICT,
    RuntimeError,
)
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.protocol import Runner, RunnerInvokeError


class PythonRunnerBackend:
    def __init__(self) -> None:
        self._runners: dict[str, Runner] = {}
        self._cancelled_invocations: set[str] = set()

    def register_runner(self, runner: Runner) -> None:
        self._runners[runner.descriptor.runner_id] = runner

    def descriptors(self) -> tuple[RunnerDescriptor, ...]:
        return tuple(runner.descriptor for runner in self._runners.values())

    async def step_runner(
        self,
        runner_id: str,
        ctx: RunnerContext,
        tasks: tuple[Task, ...],
    ) -> tuple[RunnerResult, ...]:
        for task in tasks:
            if task.lease_id != ctx.task_lease_id:
                raise RunnerInvokeError(
                    RuntimeError(
                        code=ERR_TASK_CLAIM_CONFLICT,
                        source="python_runner_backend",
                        route=f"python.runner.step.{task.task_id}",
                        evidence={
                            "ctx_task_lease_id": ctx.task_lease_id or "",
                            "task_lease_id": task.lease_id or "",
                            "executor_id": ctx.executor_id,
                        },
                    )
                )
        cancel_requested = ctx.cancel_requested or ctx.invocation_id in self._cancelled_invocations
        self._cancelled_invocations.discard(ctx.invocation_id)
        if cancel_requested != ctx.cancel_requested:
            ctx = replace(ctx, cancel_requested=True)
        return await self._runner(runner_id).step(ctx, tasks)

    async def cancel_runner(self, runner_id: str, invocation_id: str) -> None:
        self._cancelled_invocations.add(invocation_id)
        await self._runner(runner_id).cancel(invocation_id)

    async def dispose_runner(self, runner_id: str) -> None:
        await self._runner(runner_id).dispose()

    def _runner(self, runner_id: str) -> Runner:
        runner = self._runners.get(runner_id)
        if runner is None:
            raise RunnerInvokeError(
                RuntimeError(
                    code=ERR_RUNNER_NOT_FOUND,
                    source="python_runner_backend",
                    route=f"python.runner.{runner_id}",
                    evidence={"runner_id": runner_id},
                )
            )
        return runner
