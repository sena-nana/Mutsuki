from __future__ import annotations

from mutsuki_runtime_python.contracts import (
    RunnerContext,
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
    Task,
    from_json_dict,
    to_json_dict,
)


class EchoRunner:
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        self._descriptor = descriptor
        self.cancelled: list[str] = []
        self.disposed = False

    @property
    def descriptor(self) -> RunnerDescriptor:
        return self._descriptor

    async def step(
        self,
        ctx: RunnerContext,
        tasks: tuple[Task, ...],
    ) -> tuple[RunnerResult, ...]:
        _ = ctx
        return tuple(RunnerResult.completed(task.task_id) for task in tasks)

    async def cancel(self, invocation_id: str) -> None:
        self.cancelled.append(invocation_id)

    async def dispose(self) -> None:
        self.disposed = True


def echo_descriptor() -> RunnerDescriptor:
    return RunnerDescriptor(
        runner_id="echo.runner",
        plugin_id="plugin.echo",
        plugin_generation=1,
        accepted_task_kinds=("raw.input",),
        purity=RunnerPurity.PURE,
        contract_surfaces=("runner:echo.runner",),
    )


def assert_json_roundtrip[T](contract_type: type[T], value: T) -> T:
    encoded = to_json_dict(value)
    decoded = from_json_dict(contract_type, encoded)
    assert decoded == value
    return decoded
