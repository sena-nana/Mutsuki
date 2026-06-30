from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    as_int,
    as_json_value,
    as_mapping,
    as_str,
    as_str_tuple,
    field_value,
    optional_str,
    to_json_value,
    tuple_from_json,
)
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.resource import ResourceRef
from mutsuki_runner_kit.contracts.state import VersionExpectation


class TaskStatus(StrEnum):
    CREATED = "created"
    READY = "ready"
    RUNNING = "running"
    WAITING = "waiting"
    BLOCKED = "blocked"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"
    EXPIRED = "expired"
    DEAD_LETTER = "dead_letter"


class CancelPolicy(StrEnum):
    CASCADE = "cascade"
    DETACH = "detach"
    SHIELD = "shield"


@dataclass(frozen=True)
class Task:
    task_id: str
    protocol_id: str
    priority: int
    ready_at_step: int | None
    payload: JsonValue
    input_refs: tuple[str, ...]
    output_ref: str | None
    continuation_ref: str | None
    target_binding_id: str | None
    lease_id: str | None
    trace_id: str | None
    expected_versions: tuple[VersionExpectation, ...]
    correlation_id: str | None
    idempotency_key: str | None
    runner_hint: str | None
    registry_generation: int
    required_surfaces: tuple[str, ...]
    created_sequence: int

    @classmethod
    def new(cls, task_id: str, protocol_id: str, payload: JsonValue = None) -> Self:
        return cls(
            task_id=task_id,
            protocol_id=protocol_id,
            priority=0,
            ready_at_step=None,
            payload=payload,
            input_refs=(),
            output_ref=None,
            continuation_ref=None,
            target_binding_id=None,
            lease_id=None,
            trace_id=None,
            expected_versions=(),
            correlation_id=None,
            idempotency_key=None,
            runner_hint=None,
            registry_generation=0,
            required_surfaces=(),
            created_sequence=0,
        )
    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "Task")
        ready_at_step = field_value(raw, "ready_at_step")
        correlation_id = field_value(raw, "correlation_id")
        idempotency_key = field_value(raw, "idempotency_key")
        runner_hint = field_value(raw, "runner_hint")
        output_ref = field_value(raw, "output_ref")
        continuation_ref = field_value(raw, "continuation_ref")
        target_binding_id = field_value(raw, "target_binding_id")
        lease_id = field_value(raw, "lease_id")
        trace_id = field_value(raw, "trace_id")
        return cls(
            task_id=as_str(field_value(raw, "task_id"), "task_id"),
            protocol_id=as_str(field_value(raw, "protocol_id"), "protocol_id"),
            priority=as_int(field_value(raw, "priority"), "priority"),
            ready_at_step=None
            if ready_at_step is None
            else as_int(ready_at_step, "ready_at_step"),
            payload=as_json_value(field_value(raw, "payload")),
            input_refs=as_str_tuple(field_value(raw, "input_refs"), "input_refs"),
            output_ref=None if output_ref is None else as_str(output_ref, "output_ref"),
            continuation_ref=None
            if continuation_ref is None
            else as_str(continuation_ref, "continuation_ref"),
            target_binding_id=None
            if target_binding_id is None
            else as_str(target_binding_id, "target_binding_id"),
            lease_id=None if lease_id is None else as_str(lease_id, "lease_id"),
            trace_id=None if trace_id is None else as_str(trace_id, "trace_id"),
            expected_versions=tuple_from_json(raw, "expected_versions", VersionExpectation),
            correlation_id=None
            if correlation_id is None
            else as_str(correlation_id, "correlation_id"),
            idempotency_key=None
            if idempotency_key is None
            else as_str(idempotency_key, "idempotency_key"),
            runner_hint=None if runner_hint is None else as_str(runner_hint, "runner_hint"),
            registry_generation=as_int(
                field_value(raw, "registry_generation"), "registry_generation"
            ),
            required_surfaces=as_str_tuple(
                field_value(raw, "required_surfaces"), "required_surfaces"
            ),
            created_sequence=as_int(field_value(raw, "created_sequence"), "created_sequence"),
        )


@dataclass(frozen=True)
class TaskLease:
    lease_id: str
    task_id: str
    runner_id: str
    executor_id: str
    registry_generation: int
    acquired_at_step: int
    expires_at_step: int | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskLease")
        expires_at_step = field_value(raw, "expires_at_step")
        return cls(
            lease_id=as_str(field_value(raw, "lease_id"), "lease_id"),
            task_id=as_str(field_value(raw, "task_id"), "task_id"),
            runner_id=as_str(field_value(raw, "runner_id"), "runner_id"),
            executor_id=as_str(field_value(raw, "executor_id"), "executor_id"),
            registry_generation=as_int(
                field_value(raw, "registry_generation"), "registry_generation"
            ),
            acquired_at_step=as_int(field_value(raw, "acquired_at_step"), "acquired_at_step"),
            expires_at_step=None
            if expires_at_step is None
            else as_int(expires_at_step, "expires_at_step"),
        )


@dataclass(frozen=True)
class TaskHandle:
    task_id: str
    protocol_id: str
    target_binding_id: str | None
    cancel_policy: CancelPolicy
    trace_id: str | None
    correlation_id: str | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskHandle")
        return cls(
            task_id=as_str(field_value(raw, "task_id"), "task_id"),
            protocol_id=as_str(field_value(raw, "protocol_id"), "protocol_id"),
            target_binding_id=optional_str(
                field_value(raw, "target_binding_id"), "target_binding_id"
            ),
            cancel_policy=CancelPolicy(as_str(field_value(raw, "cancel_policy"), "cancel_policy")),
            trace_id=optional_str(field_value(raw, "trace_id"), "trace_id"),
            correlation_id=optional_str(field_value(raw, "correlation_id"), "correlation_id"),
        )


@dataclass(frozen=True)
class TaskOutcome:
    status: TaskStatus
    task_id: str
    output_ref: str | None = None
    error: RuntimeError | None = None
    reason: str | None = None

    @classmethod
    def completed(cls, task_id: str, output_ref: str | None = None) -> Self:
        return cls(status=TaskStatus.COMPLETED, task_id=task_id, output_ref=output_ref)

    @classmethod
    def failed(cls, task_id: str, error: RuntimeError) -> Self:
        return cls(status=TaskStatus.FAILED, task_id=task_id, error=error)

    @classmethod
    def cancelled(cls, task_id: str, reason: str | None = None) -> Self:
        return cls(status=TaskStatus.CANCELLED, task_id=task_id, reason=reason)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskOutcome")
        status = TaskStatus(as_str(field_value(raw, "status"), "status"))
        task_id = as_str(field_value(raw, "task_id"), "task_id")
        if status == TaskStatus.COMPLETED:
            return cls.completed(
                task_id,
                optional_str(field_value(raw, "output_ref"), "output_ref"),
            )
        if status == TaskStatus.FAILED:
            return cls.failed(
                task_id,
                RuntimeError.from_json_dict(as_mapping(field_value(raw, "error"), "error")),
            )
        return cls(
            status=status,
            task_id=task_id,
            reason=optional_str(field_value(raw, "reason"), "reason"),
        )

    def to_json_value(self) -> JsonDict:
        if self.status == TaskStatus.COMPLETED:
            return {
                "status": self.status.value,
                "task_id": self.task_id,
                "output_ref": self.output_ref,
            }
        if self.status == TaskStatus.FAILED:
            if self.error is None:
                raise TypeError("error is required for failed TaskOutcome")
            return {
                "status": self.status.value,
                "task_id": self.task_id,
                "error": to_json_value(self.error),
            }
        return {
            "status": self.status.value,
            "task_id": self.task_id,
            "reason": self.reason,
        }


@dataclass(frozen=True)
class WakeCondition:
    type: str
    ready_at_step: int | None = None
    ref_id: str | None = None
    signal_id: str | None = None

    @classmethod
    def timer(cls, ready_at_step: int) -> Self:
        return cls(type="timer", ready_at_step=ready_at_step)

    @classmethod
    def retry_after(cls, ready_at_step: int) -> Self:
        return cls(type="retry_after", ready_at_step=ready_at_step)

    @classmethod
    def resource_event(cls, ref_id: str) -> Self:
        return cls(type="resource_event", ref_id=ref_id)

    @classmethod
    def external_signal(cls, signal_id: str) -> Self:
        return cls(type="external_signal", signal_id=signal_id)

    @classmethod
    def manual_wake(cls) -> Self:
        return cls(type="manual_wake")

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "WakeCondition")
        kind = as_str(field_value(raw, "type"), "type")
        if kind in ("timer", "retry_after"):
            return cls(
                type=kind,
                ready_at_step=as_int(field_value(raw, "ready_at_step"), "ready_at_step"),
            )
        if kind == "resource_event":
            return cls.resource_event(as_str(field_value(raw, "ref_id"), "ref_id"))
        if kind == "external_signal":
            return cls.external_signal(as_str(field_value(raw, "signal_id"), "signal_id"))
        if kind == "manual_wake":
            return cls.manual_wake()
        raise TypeError(f"unknown WakeCondition type: {kind}")

    def to_json_value(self) -> JsonDict:
        if self.type in ("timer", "retry_after"):
            if self.ready_at_step is None:
                raise TypeError("ready_at_step is required for timer wake")
            return {"type": self.type, "ready_at_step": self.ready_at_step}
        if self.type == "resource_event":
            if self.ref_id is None:
                raise TypeError("ref_id is required for resource_event wake")
            return {"type": self.type, "ref_id": self.ref_id}
        if self.type == "external_signal":
            if self.signal_id is None:
                raise TypeError("signal_id is required for external_signal wake")
            return {"type": self.type, "signal_id": self.signal_id}
        if self.type == "manual_wake":
            return {"type": self.type}
        raise TypeError(f"unknown WakeCondition type: {self.type}")


@dataclass(frozen=True)
class TaskStepContinuation:
    continuation: ResourceRef
    wake: WakeCondition | None
    reason: str | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskStepContinuation")
        wake = field_value(raw, "wake")
        return cls(
            continuation=ResourceRef.from_json_dict(
                as_mapping(field_value(raw, "continuation"), "continuation")
            ),
            wake=None if wake is None else WakeCondition.from_json_dict(as_mapping(wake, "wake")),
            reason=optional_str(field_value(raw, "reason"), "reason"),
        )


@dataclass(frozen=True)
class TaskAwait:
    parent_task_id: str
    child: TaskHandle
    continuation: TaskStepContinuation
    cancel_policy: CancelPolicy

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskAwait")
        return cls(
            parent_task_id=as_str(field_value(raw, "parent_task_id"), "parent_task_id"),
            child=TaskHandle.from_json_dict(as_mapping(field_value(raw, "child"), "child")),
            continuation=TaskStepContinuation.from_json_dict(
                as_mapping(field_value(raw, "continuation"), "continuation")
            ),
            cancel_policy=CancelPolicy(as_str(field_value(raw, "cancel_policy"), "cancel_policy")),
        )
