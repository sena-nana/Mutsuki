from __future__ import annotations

from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.event import (
    RuntimeEvent,
    RuntimeEventKind,
    SpanStatus,
    TraceSpan,
)
from mutsuki_runner_kit.testing.assertions import assert_json_roundtrip


def test_error_event_and_trace_contracts_roundtrip() -> None:
    error = RuntimeError(
        code="runtime.test_failed",
        source="contracts.test",
        route="test.route",
        evidence={"attempt": 1},
    )
    assert_json_roundtrip(RuntimeError, error)

    event = RuntimeEvent(
        sequence=7,
        kind=RuntimeEventKind.TRACE,
        name="trace.closed",
        subject_id="trace-1",
        attributes={"ok": True},
        error=error,
    )
    assert_json_roundtrip(RuntimeEvent, event)

    span = TraceSpan(
        trace_id="trace-1",
        span_id="span-1",
        parent_span_id=None,
        name="runner.step",
        start=1.0,
        end=2.0,
        attributes={"runner_id": "worker"},
        status=SpanStatus.OK,
    )
    assert_json_roundtrip(TraceSpan, span)
