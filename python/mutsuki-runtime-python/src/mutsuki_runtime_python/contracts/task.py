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
    kind: str
    priority: int
    ready_at_step: int | None
    payload: JsonValue
    input_refs: tuple[str, ...]
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
            kind=kind,
            priority=0,
            ready_at_step=None,
            payload=payload,
            input_refs=(),
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
        return cls(
            task_id=as_str(field_value(raw, "task_id"), "task_id"),
            kind=as_str(field_value(raw, "kind"), "kind"),
            priority=as_int(field_value(raw, "priority"), "priority"),
            ready_at_step=None
            if ready_at_step is None
            else as_int(ready_at_step, "ready_at_step"),
            payload=as_json_value(field_value(raw, "payload")),
            input_refs=as_str_tuple(field_value(raw, "input_refs"), "input_refs"),
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
