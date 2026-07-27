from __future__ import annotations

from dataclasses import replace

from mutsuki_runner_kit.contracts.batch import CompletionBatch, WorkBatch
from mutsuki_runner_kit.contracts.errors import (
    ERR_CAPABILITY_EXHAUSTED,
    ERR_RUNNER_NOT_FOUND,
    ERR_TASK_CLAIM_CONFLICT,
    RuntimeError,
)
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor
from mutsuki_runner_kit.runners.protocol import Runner, RunnerInvokeError


class PythonRunnerBackend:
    def __init__(self, *, max_pending_cancels: int = 64) -> None:
        if max_pending_cancels <= 0:
            raise ValueError("max_pending_cancels must be positive")
        self._runners: dict[str, Runner] = {}
        self._cancelled_invocations: set[tuple[str, str]] = set()
        self._active_invocations: set[tuple[str, str]] = set()
        self._max_pending_cancels = max_pending_cancels

    def register_runner(self, runner: Runner) -> None:
        self._runners[runner.descriptor.runner_id] = runner

    def descriptors(self) -> tuple[RunnerDescriptor, ...]:
        return tuple(runner.descriptor for runner in self._runners.values())

    async def run_batch_runner(
        self,
        runner_id: str,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> CompletionBatch:
        batch_lease_ids = tuple(lease.lease_id for lease in batch.task_leases)
        if batch_lease_ids != ctx.task_lease_ids:
            raise RunnerInvokeError(
                RuntimeError(
                    code=ERR_TASK_CLAIM_CONFLICT,
                    source="python_runner_backend",
                    route=f"python.runner.run_batch.{batch.batch_id}",
                    evidence={
                        "ctx_task_lease_ids": ",".join(ctx.task_lease_ids),
                        "batch_task_lease_ids": ",".join(batch_lease_ids),
                        "executor_id": ctx.executor_id,
                    },
                )
            )
        invocation = (runner_id, ctx.invocation_id)
        if invocation in self._active_invocations:
            raise RunnerInvokeError(
                RuntimeError(
                    code=ERR_TASK_CLAIM_CONFLICT,
                    source="python_runner_backend",
                    route=f"python.runner.invocation.{ctx.invocation_id}",
                    evidence={"runner_id": runner_id, "invocation_id": ctx.invocation_id},
                )
            )
        cancel_requested = ctx.cancel_requested or invocation in self._cancelled_invocations
        self._cancelled_invocations.discard(invocation)
        if cancel_requested != ctx.cancel_requested:
            ctx = replace(ctx, cancel_requested=True)
        runner = self._runner(runner_id)
        self._active_invocations.add(invocation)
        try:
            return await runner.run_batch(ctx, batch)
        finally:
            self._active_invocations.discard(invocation)

    async def cancel_runner(self, runner_id: str, invocation_id: str) -> None:
        runner = self._runner(runner_id)
        invocation = (runner_id, invocation_id)
        if (
            invocation not in self._active_invocations
            and invocation not in self._cancelled_invocations
            and len(self._cancelled_invocations) >= self._max_pending_cancels
        ):
            raise RunnerInvokeError(
                RuntimeError(
                    code=ERR_CAPABILITY_EXHAUSTED,
                    source="python_runner_backend",
                    route="python.runner.pending_cancel",
                    evidence={"limit": self._max_pending_cancels},
                )
            )
        if invocation not in self._active_invocations:
            self._cancelled_invocations.add(invocation)
        await runner.cancel(invocation_id)

    async def dispose_runner(self, runner_id: str) -> None:
        runner = self._runner(runner_id)
        self._cancelled_invocations = {
            invocation
            for invocation in self._cancelled_invocations
            if invocation[0] != runner_id
        }
        await runner.dispose()

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
