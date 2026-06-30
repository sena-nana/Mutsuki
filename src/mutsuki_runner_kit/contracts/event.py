from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    ScalarValue,
    as_int,
    as_json_value,
    as_mapping,
    as_scalar,
    as_scalar_dict,
    as_str,
    field_value,
    optional_str,
)
from mutsuki_runner_kit.contracts.errors import RuntimeError


class RuntimeEventKind(StrEnum):
    LIFECYCLE = "lifecycle"
    PLUGIN = "plugin"
    RESOURCE = "resource"
    TRACE = "trace"
    HOST = "host"
    TASK = "task"
    RUNNER = "runner"
    STATE = "state"
    EFFECT = "effect"
    RELOAD = "reload"


class SpanStatus(StrEnum):
    OK = "ok"
    ERROR = "error"


@dataclass(frozen=True)
class DomainEvent:
    event_id: str
    kind: str
    payload: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "DomainEvent")
        return cls(
            event_id=as_str(field_value(raw, "event_id"), "event_id"),
            kind=as_str(field_value(raw, "kind"), "kind"),
            payload=as_json_value(field_value(raw, "payload")),
        )


@dataclass(frozen=True)
class RuntimeEvent:
    sequence: int
    kind: RuntimeEventKind
    name: str
    subject_id: str | None
    attributes: dict[str, ScalarValue]
    error: RuntimeError | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RuntimeEvent")
        return cls(
            sequence=as_int(field_value(raw, "sequence"), "sequence"),
            kind=RuntimeEventKind(as_str(field_value(raw, "kind"), "kind")),
            name=as_str(field_value(raw, "name"), "name"),
            subject_id=optional_str(field_value(raw, "subject_id"), "subject_id"),
            attributes=as_scalar_dict(field_value(raw, "attributes"), "attributes"),
            error=None
            if field_value(raw, "error") is None
            else RuntimeError.from_json_dict(as_mapping(field_value(raw, "error"), "error")),
        )


@dataclass(frozen=True)
class TraceSpan:
    trace_id: str
    span_id: str
    parent_span_id: str | None
    name: str
    start: float
    end: float | None
    attributes: dict[str, ScalarValue]
    status: SpanStatus

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "TraceSpan")
        start = field_value(raw, "start")
        end = field_value(raw, "end")
        return cls(
            trace_id=as_str(field_value(raw, "trace_id"), "trace_id"),
            span_id=as_str(field_value(raw, "span_id"), "span_id"),
            parent_span_id=optional_str(field_value(raw, "parent_span_id"), "parent_span_id"),
            name=as_str(field_value(raw, "name"), "name"),
            start=float(as_scalar(start, "start")),
            end=None if end is None else float(as_scalar(end, "end")),
            attributes=as_scalar_dict(field_value(raw, "attributes"), "attributes"),
            status=SpanStatus(as_str(field_value(raw, "status"), "status")),
        )
