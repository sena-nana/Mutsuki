from __future__ import annotations

from typing import Protocol

from mutsuki_runtime_python.contracts import (
    RunnerContext,
    RunnerDescriptor,
    RunnerResult,
    RuntimeError,
    Task,
)


class RunnerInvokeError(Exception):
    def __init__(self, error: RuntimeError) -> None:
        super().__init__(f"runtime runner failed: {error.code}")
        self.error = error


class Runner(Protocol):
    @property
    def descriptor(self) -> RunnerDescriptor: ...

    async def step(
        self, ctx: RunnerContext, tasks: tuple[Task, ...]
    ) -> tuple[RunnerResult, ...]: ...

    async def cancel(self, invocation_id: str) -> None: ...

    async def dispose(self) -> None: ...
