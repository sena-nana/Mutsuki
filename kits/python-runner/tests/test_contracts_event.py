from __future__ import annotations

from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.event import (
    RuntimeEvent,
    RuntimeEventKind,
    SpanStatus,
    TraceSpan,
)
from mutsuki_runner_kit.contracts.observability import (
    ObservabilityOutletProfile,
    ObservabilityOverflowPolicy,
    ObservabilityPage,
    ObservabilityProfile,
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
        sequence=1,
        trace_id="trace-1",
        span_id="span-1",
        parent_span_id=None,
        name="runner.run_batch",
        start=1.0,
        end=2.0,
        attributes={"runner_id": "worker"},
        status=SpanStatus.OK,
    )
    assert_json_roundtrip(TraceSpan, span)


def test_observability_profile_and_cursor_page_roundtrip() -> None:
    profile = ObservabilityProfile(
        events=ObservabilityOutletProfile(
            capacity=128,
            overflow_policy=ObservabilityOverflowPolicy.DROP_NEW,
        ),
        traces=ObservabilityOutletProfile(
            capacity=64,
            overflow_policy=ObservabilityOverflowPolicy.DROP_OLDEST,
        ),
        detailed_scheduler_decisions=True,
        dispatch_spans=True,
    )
    assert_json_roundtrip(ObservabilityProfile, profile)

    page = ObservabilityPage(
        items=("span-4",),
        next_sequence=4,
        earliest_available_sequence=4,
        latest_sequence=5,
        lost=3,
        truncated=True,
        dropped=3,
    )
    assert_json_roundtrip(ObservabilityPage, page)
    assert page.cursor_lost()
