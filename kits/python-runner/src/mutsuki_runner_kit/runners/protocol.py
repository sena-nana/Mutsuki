from __future__ import annotations

from typing import Protocol

from mutsuki_runner_kit.contracts.batch import CompletionBatch, WorkBatch
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor


class RunnerInvokeError(Exception):
    def __init__(self, error: RuntimeError) -> None:
        super().__init__(f"runtime runner failed: {error.code}")
        self.error = error


class Runner(Protocol):
    @property
    def descriptor(self) -> RunnerDescriptor: ...

    async def run_batch(self, ctx: RunnerContext, batch: WorkBatch) -> CompletionBatch: ...

    async def cancel(self, invocation_id: str) -> None: ...

    async def dispose(self) -> None: ...
