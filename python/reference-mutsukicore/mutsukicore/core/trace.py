"""Core trace helpers.

Core code emits trace spans through ``ctx.bus`` only. Observability remains a
sidecar subscriber and is not imported here.
"""

from __future__ import annotations

from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from mutsukicore.contracts.event import SpanStatus, TraceSpan
from mutsukicore.contracts.ids import SpanId
from mutsukicore.core.context import AgentContext

TraceAttrs = dict[str, str | int | float | bool]


@asynccontextmanager
async def trace_span(
    ctx: AgentContext,
    name: str,
    *,
    attributes: TraceAttrs | None = None,
) -> AsyncIterator[TraceSpan]:
    """Create a child span around an inline awaitable block.

    The active ``ctx.trace_ctx.span_id`` is temporarily replaced by the child
    span, so nested dispatcher/resource calls keep a real parent-child chain.
    """

    parent_span_id = ctx.trace_ctx.span_id
    span_id = SpanId(ctx.id_gen.next("span"))
    start = ctx.clock.now()
    old_span_id = ctx.trace_ctx.span_id
    old_parent_span_id = ctx.trace_ctx.parent_span_id
    ctx.trace_ctx.span_id = span_id
    ctx.trace_ctx.parent_span_id = parent_span_id
    span = TraceSpan(
        trace_id=ctx.trace_ctx.trace_id,
        span_id=span_id,
        parent_span_id=parent_span_id,
        name=name,
        start=start,
        attributes=attributes or {},
    )
    try:
        yield span
    except Exception:
        span.status = SpanStatus.ERROR
        raise
    finally:
        span.end = ctx.clock.now()
        ctx.trace_ctx.span_id = old_span_id
        ctx.trace_ctx.parent_span_id = old_parent_span_id
        await ctx.bus.publish("trace.span", span)


__all__ = ["TraceAttrs", "trace_span"]
