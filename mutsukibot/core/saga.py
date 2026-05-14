"""Saga：多步事务，每步自带补偿动作。"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any

ForwardFn = Callable[[], Awaitable[Any]]
CompensateFn = Callable[[], Awaitable[None]]


@dataclass(slots=True)
class _Step:
    forward: ForwardFn
    compensate: CompensateFn


class SagaCompensationError(Exception):
    """补偿动作本身失败时抛出（携带原始错误链）。"""

    def __init__(self, original: BaseException, comp_errors: list[BaseException]) -> None:
        super().__init__(
            f"主错误后补偿失败: {original!r}; 补偿错误: {comp_errors!r}"
        )
        self.original = original
        self.comp_errors = comp_errors


@dataclass(slots=True)
class Saga:
    _steps: list[_Step] = field(default_factory=list)

    def add_step(self, forward: ForwardFn, compensate: CompensateFn) -> None:
        self._steps.append(_Step(forward, compensate))

    async def run(self) -> list[Any]:
        results: list[Any] = []
        completed: list[_Step] = []
        try:
            for step in self._steps:
                results.append(await step.forward())
                completed.append(step)
            return results
        except BaseException as exc:
            comp_errors: list[BaseException] = []
            for step in reversed(completed):
                try:
                    await step.compensate()
                except BaseException as ce:
                    comp_errors.append(ce)
            if comp_errors:
                raise SagaCompensationError(exc, comp_errors) from exc
            raise


__all__ = ["Saga", "SagaCompensationError"]
