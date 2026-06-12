"""内部事件与 trace span 契约。"""

from __future__ import annotations

from enum import StrEnum
from typing import ClassVar

import msgspec

from mutsuki.contracts.base import Contract
from mutsuki.contracts.ids import SpanId, TraceId


class SpanStatus(StrEnum):
    OK = "ok"
    ERROR = "error"


class TraceSpan(Contract):
    """因果链上的 trace span。"""

    schema_id: ClassVar[str] = "mutsuki.trace_span"
    schema_version: ClassVar[str] = "1.0.0"

    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None
    name: str = ""
    start: float = 0.0
    end: float | None = None
    attributes: dict[str, str | int | float | bool] = {}
    status: SpanStatus = SpanStatus.OK


class Event(Contract):
    """跨插件的内部事件。"""

    schema_id: ClassVar[str] = "mutsuki.event"
    schema_version: ClassVar[str] = "1.0.0"

    id: str
    timestamp: float
    type: str
    source_plugin: str
    payload: msgspec.Raw
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None


__all__ = ["Event", "SpanStatus", "TraceSpan"]
