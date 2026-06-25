from __future__ import annotations

from mutsuki_runtime_python.contracts.errors import ERR_RUNNER_NOT_FOUND, RuntimeError
from mutsuki_runtime_python.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runtime_python.contracts.task import Task
from mutsuki_runtime_python.runners.protocol import Runner, RunnerInvokeError


class PythonRunnerHost:
    def __init__(self) -> None:
        self._runners: dict[str, Runner] = {}

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
        return await self._runner(runner_id).step(ctx, tasks)

    async def cancel_runner(self, runner_id: str, invocation_id: str) -> None:
        await self._runner(runner_id).cancel(invocation_id)

    async def dispose_runner(self, runner_id: str) -> None:
        await self._runner(runner_id).dispose()

    def _runner(self, runner_id: str) -> Runner:
        runner = self._runners.get(runner_id)
        if runner is None:
            raise RunnerInvokeError(
                RuntimeError(
                    code=ERR_RUNNER_NOT_FOUND,
                    source="python_runner_host",
                    route=f"python.runner.{runner_id}",
                    evidence={"runner_id": runner_id},
                )
            )
        return runner
