"""Typed wrappers that keep protocol identity next to untyped wire DTOs."""

from __future__ import annotations

from mutsuki_runner_kit.contracts.codec import JsonValue
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.task import TaskHandle, TaskOutcome, TaskStatus
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError

_SDK_SOURCE = "runtime.sdk"


class TypedTaskHandle:
    def __init__(self, handle: TaskHandle, protocol_id: str) -> None:
        if handle.protocol_id != protocol_id:
            raise RunnerInvokeError(
                RuntimeError(
                    code="task.protocol_mismatch",
                    source=_SDK_SOURCE,
                    route=f"task.handle.{handle.task_id}",
                    evidence={
                        "expected_protocol_id": protocol_id,
                        "actual_protocol_id": handle.protocol_id,
                    },
                )
            )
        self._handle = handle

    @property
    def protocol_id(self) -> str:
        return self._handle.protocol_id

    def as_handle(self) -> TaskHandle:
        return self._handle


class TypedTaskOutcome:
    def __init__(self, outcome: TaskOutcome) -> None:
        self._outcome = outcome

    def as_outcome(self) -> TaskOutcome:
        return self._outcome

    def decode(self) -> JsonValue:
        if self._outcome.status == TaskStatus.FAILED and self._outcome.error is not None:
            raise RunnerInvokeError(self._outcome.error)
        if self._outcome.status != TaskStatus.COMPLETED or self._outcome.output is None:
            raise RunnerInvokeError(
                RuntimeError(
                    code="sdk.decode_failed",
                    source=_SDK_SOURCE,
                    route=f"task.outcome.{self._outcome.task_id}.{self._outcome.status.value}",
                )
            )
        return self._outcome.output
