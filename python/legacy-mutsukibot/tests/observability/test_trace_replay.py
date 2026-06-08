from __future__ import annotations

import json

import pytest

from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.event import SpanStatus, TraceSpan
from mutsukibot.contracts.ids import SpanId, TraceId
from mutsukibot.core.bus import Bus
from mutsukibot.observability.trace import JsonlTraceReader, JsonlTraceWriter
from mutsukibot.testing.trace_replay import TraceReplayError, replay_trace_spans


def _span(
    span_id: str,
    *,
    parent: str | None = None,
    name: str = "test.span",
    start: float = 0.0,
    end: float = 1.0,
) -> TraceSpan:
    return TraceSpan(
        trace_id=TraceId("trace-1"),
        span_id=SpanId(span_id),
        parent_span_id=SpanId(parent) if parent is not None else None,
        name=name,
        start=start,
        end=end,
        status=SpanStatus.OK,
        attributes={"phase": name},
    )


@pytest.mark.asyncio
async def test_jsonl_trace_writer_reader_round_trips_replayable_spans(tmp_path) -> None:
    path = tmp_path / "trace.jsonl"
    bus = Bus()
    writer = JsonlTraceWriter(path)
    writer.attach(bus)

    await bus.publish("trace.span", _span("root", name="dispatch.invoke", start=0.0, end=3.0))
    await bus.publish(
        "trace.span",
        _span("child", parent="root", name="resource_host.acquire", start=1.0, end=2.0),
    )
    writer.detach()

    spans = JsonlTraceReader(path).read_all()
    frames = replay_trace_spans(spans, require_known_parents=True)

    assert [span.name for span in spans] == ["dispatch.invoke", "resource_host.acquire"]
    assert [(frame.span.name, frame.depth) for frame in frames] == [
        ("dispatch.invoke", 0),
        ("resource_host.acquire", 1),
    ]
    assert frames[1].parent_span_id == SpanId("root")


def test_jsonl_trace_reader_reports_invalid_record_as_structured_error(tmp_path) -> None:
    path = tmp_path / "trace.jsonl"
    path.write_text(json.dumps({"trace_id": "trace-1", "name": "broken"}) + "\n", encoding="utf-8")

    with pytest.raises(TraceReplayError) as exc:
        JsonlTraceReader(path).read_all()

    assert exc.value.error.code == Errs.TRACE_RECORD_INVALID
    assert exc.value.error.evidence["line"] == 1
    assert exc.value.error.evidence["path"] == str(path)


def test_replay_trace_spans_rejects_duplicate_span_ids() -> None:
    spans = (
        _span("dup", name="first", start=0.0),
        _span("dup", name="second", start=1.0),
    )

    with pytest.raises(TraceReplayError) as exc:
        replay_trace_spans(spans)

    assert exc.value.error.code == Errs.TRACE_REPLAY_FAILED
    assert exc.value.error.evidence["span_id"] == "dup"
