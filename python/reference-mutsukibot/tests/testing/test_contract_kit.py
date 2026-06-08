from __future__ import annotations

import pytest

from mutsukibot.contracts.event import SpanStatus, TraceSpan
from mutsukibot.contracts.ids import SpanId, TraceId
from mutsukibot.testing.contract_kit import (
    assert_cross_agent_trace_chain,
    assert_dispatcher_clean,
)


def _span(
    span_id: str,
    *,
    parent: str | None = None,
    name: str = "test.span",
) -> TraceSpan:
    return TraceSpan(
        trace_id=TraceId("trace-1"),
        span_id=SpanId(span_id),
        parent_span_id=SpanId(parent) if parent is not None else None,
        name=name,
        start=0.0,
        end=1.0,
        status=SpanStatus.OK,
        attributes={"phase": name},
    )


def test_assert_cross_agent_trace_chain_links_dispatch_spans() -> None:
    caller = _span("caller", name="dispatch.invoke_in_agent")
    target = _span("target", parent="caller", name="dispatch.invoke")

    link = assert_cross_agent_trace_chain([caller, target])

    assert link.caller.span_id == SpanId("caller")
    assert link.target.parent_span_id == SpanId("caller")


def test_assert_dispatcher_clean_passes_for_empty_dispatch() -> None:
    class _Dispatch:
        def list_operations(self) -> tuple[str, ...]:
            return ()

        def list_sources(self) -> tuple[str, ...]:
            return ()

    class _Agent:
        dispatch = _Dispatch()

    assert_dispatcher_clean(_Agent())


def test_assert_dispatcher_clean_rejects_residual_registration() -> None:
    class _Dispatch:
        def list_operations(self) -> tuple[str, ...]:
            return ("test.op",)

        def list_sources(self) -> tuple[str, ...]:
            return ()

    class _Agent:
        dispatch = _Dispatch()

    with pytest.raises(AssertionError):
        assert_dispatcher_clean(_Agent())
