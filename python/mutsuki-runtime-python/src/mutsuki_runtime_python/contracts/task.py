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
    required_optional,
    tuple_from_json,
)
from mutsuki_runtime_python.contracts.state import VersionExpectation


class TaskStatus(StrEnum):
    PENDING = "pending"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


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


@dataclass(frozen=True)
class TaskMatchRule:
    type: str
    kind: str | None = None
    prefix: str | None = None

    @classmethod
    def kind_rule(cls, kind: str) -> Self:
        return cls(type="kind", kind=kind)

    @classmethod
    def kind_prefix(cls, prefix: str) -> Self:
        return cls(type="kind_prefix", prefix=prefix)

    @classmethod
    def any(cls) -> Self:
        return cls(type="any")

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskMatchRule")
        rule_type = as_str(field_value(raw, "type"), "type")
        if rule_type == "kind":
            return cls.kind_rule(as_str(field_value(raw, "kind"), "kind"))
        if rule_type == "kind_prefix":
            return cls.kind_prefix(as_str(field_value(raw, "prefix"), "prefix"))
        if rule_type == "any":
            return cls.any()
        raise ValueError(f"unknown task match rule type: {rule_type}")

    def to_json_value(self) -> JsonDict:
        if self.type == "kind":
            return {"type": "kind", "kind": required_optional(self.kind, "kind")}
        if self.type == "kind_prefix":
            return {"type": "kind_prefix", "prefix": required_optional(self.prefix, "prefix")}
        if self.type == "any":
            return {"type": "any"}
        raise ValueError(f"unknown task match rule type: {self.type}")


@dataclass(frozen=True)
class TaskDemand:
    demand_id: str
    plugin_id: str
    match_rule: TaskMatchRule
    target_task_kind: str
    target_runner_hint: str | None
    priority: int
    payload_projection: JsonValue
    input_ref_policy: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TaskDemand")
        target_runner_hint = field_value(raw, "target_runner_hint")
        return cls(
            demand_id=as_str(field_value(raw, "demand_id"), "demand_id"),
            plugin_id=as_str(field_value(raw, "plugin_id"), "plugin_id"),
            match_rule=TaskMatchRule.from_json_dict(
                as_mapping(field_value(raw, "match_rule"), "match_rule")
            ),
            target_task_kind=as_str(field_value(raw, "target_task_kind"), "target_task_kind"),
            target_runner_hint=None
            if target_runner_hint is None
            else as_str(target_runner_hint, "target_runner_hint"),
            priority=as_int(field_value(raw, "priority"), "priority"),
            payload_projection=as_json_value(field_value(raw, "payload_projection")),
            input_ref_policy=as_str(field_value(raw, "input_ref_policy"), "input_ref_policy"),
        )
