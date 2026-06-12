"""Shared Python envelope fan-out for scheduler and runtime backend adapters."""

from __future__ import annotations

from typing import TYPE_CHECKING

from mutsuki.contracts.event import SpanStatus
from mutsuki.core.trace import trace_span

if TYPE_CHECKING:
    from mutsuki.contracts.envelope import Envelope
    from mutsuki.core.agent import Agent


async def dispatch_envelope_to_consumers(agent: "Agent", envelope: "Envelope") -> int:
    """Fan out an envelope to matching plugin consumers.

    This helper is the single Python-side implementation for
    ``Plugin.consumes`` routing. The compatibility scheduler and the
    Rust/Python backend adapter both call it.
    """

    dispatched = 0
    for entry in agent.plugins:
        plugin = entry.plugin
        consumes: tuple = plugin.__class__.consumes
        if not consumes:
            continue
        if not any(rule.check(envelope) for rule in consumes):
            continue
        dispatched += 1
        attributes: dict[str, str | int | float | bool] = {
            "agent_id": str(agent.agent_id),
            "envelope_id": str(envelope.id),
            "envelope_schema": envelope.payload_schema_id,
            "source_id": envelope.source.source_id,
        }
        ctx = agent.make_context()
        async with trace_span(
            ctx,
            f"plugin.{plugin.id}.on_envelope",
            attributes=attributes,
        ) as span:
            try:
                await plugin.on_envelope(envelope)
            except Exception as exc:
                span.status = SpanStatus.ERROR
                span.attributes["exception_type"] = type(exc).__qualname__
                span.attributes["exception_repr"] = repr(exc)
    return dispatched


__all__ = ["dispatch_envelope_to_consumers"]
