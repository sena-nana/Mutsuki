from __future__ import annotations

import pytest

from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.ids import ProtocolId, TaskId
from mutsuki_runner_kit.contracts.task import CancelPolicy, TaskHandle, TaskOutcome
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError
from mutsuki_runner_kit.tasks import TypedTaskHandle, TypedTaskOutcome


def sample_handle(protocol_id: str) -> TaskHandle:
    return TaskHandle(
        task_id=TaskId("task-1"),
        protocol_id=ProtocolId(protocol_id),
        target_binding_id=None,
        cancel_policy=CancelPolicy.CASCADE,
        trace_id=None,
        correlation_id=None,
    )


def test_typed_task_handle_rejects_protocol_mismatch() -> None:
    with pytest.raises(RunnerInvokeError) as exc_info:
        TypedTaskHandle(sample_handle("other.work"), "child.work")

    assert exc_info.value.error.code == "task.protocol_mismatch"
    assert exc_info.value.error.source == "runtime.sdk"

    typed = TypedTaskHandle(sample_handle("child.work"), "child.work")
    assert typed.as_handle().protocol_id == "child.work"
    assert typed.protocol_id == "child.work"


def test_typed_task_outcome_decodes_completed_json_output() -> None:
    outcome = TypedTaskOutcome(TaskOutcome.completed("task-1", output={"from": "parent-1"}))

    assert outcome.decode() == {"from": "parent-1"}
    assert outcome.as_outcome().task_id == "task-1"


def test_typed_task_outcome_decode_fails_for_failed_and_cancelled() -> None:
    failed = TypedTaskOutcome(
        TaskOutcome.failed(
            "task-1",
            RuntimeError(code="child.failed", source="runtime.test", route="task.outcome.task-1"),
        )
    )
    cancelled = TypedTaskOutcome(TaskOutcome.cancelled("task-1", "parent cancelled"))
    missing = TypedTaskOutcome(TaskOutcome.completed("task-1"))

    with pytest.raises(RunnerInvokeError) as failed_info:
        failed.decode()
    assert failed_info.value.error.code == "child.failed"
    assert failed_info.value.error.source == "runtime.test"

    for outcome in (cancelled, missing):
        with pytest.raises(RunnerInvokeError) as exc_info:
            outcome.decode()
        assert exc_info.value.error.code == "sdk.decode_failed"
        assert exc_info.value.error.source == "runtime.sdk"
