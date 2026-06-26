from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runtime_python.contracts.codec import (
    JsonDict,
    JsonValue,
    as_int,
    as_json_value,
    as_mapping,
    as_str,
    as_str_tuple,
    field_value,
    tuple_from_json,
)
from mutsuki_runtime_python.contracts.state import VersionExpectation


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
    def new(cls, task_id: str, kind: str, payload: JsonValue = None) -> Self:
        return cls(
            task_id=task_id,
            protocol_id=kind,
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
