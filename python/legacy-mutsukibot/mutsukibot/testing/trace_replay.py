"""Trace replay contract helpers.

The helpers validate recorded :class:`TraceSpan` sequences without re-running
side effects. They are intended for contract tests that need to prove trace
causality is complete enough to inspect or replay deterministically.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass

from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.event import SpanStatus, TraceSpan
from mutsukibot.contracts.ids import SpanId, TraceId
from mutsukibot.observability.trace import TraceReplayError

_SpanKey = tuple[str, str]


@dataclass(frozen=True, slots=True)
class TraceReplayFrame:
    span: TraceSpan
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None
    name: str
    status: SpanStatus
    depth: int


def replay_trace_spans(
    spans: Iterable[TraceSpan],
    *,
    require_known_parents: bool = False,
) -> tuple[TraceReplayFrame, ...]:
    """Validate spans and return deterministic replay frames.

    ``require_known_parents=False`` keeps single-agent trace files usable when
    their parent span is owned by an upstream caller. Contract tests that expect
    a closed trace tree can set it to ``True``.
    """

    span_list = tuple(spans)
    by_key: dict[_SpanKey, TraceSpan] = {}
    for span in span_list:
        key = _key(span)
        if key in by_key:
            _raise_replay_error(
                route="trace_replay.duplicate_span",
                evidence={
                    "trace_id": str(span.trace_id),
                    "span_id": str(span.span_id),
                },
            )
        if span.end is not None and span.end < span.start:
            _raise_replay_error(
                route="trace_replay.invalid_timing",
                evidence={
                    "trace_id": str(span.trace_id),
                    "span_id": str(span.span_id),
                    "start": span.start,
                    "end": span.end,
                },
            )
        by_key[key] = span

    depths: dict[_SpanKey, int] = {}
    visiting: set[_SpanKey] = set()

    def resolve_depth(span: TraceSpan) -> int:
        key = _key(span)
        existing = depths.get(key)
        if existing is not None:
            return existing
        if key in visiting:
            _raise_replay_error(
                route="trace_replay.parent_cycle",
                evidence={
                    "trace_id": str(span.trace_id),
                    "span_id": str(span.span_id),
                },
            )

        visiting.add(key)
        parent_id = span.parent_span_id
        if parent_id is None:
            depth = 0
        else:
            parent = by_key.get((str(span.trace_id), str(parent_id)))
            if parent is None:
                if require_known_parents:
                    _raise_replay_error(
                        route="trace_replay.parent_missing",
                        evidence={
                            "trace_id": str(span.trace_id),
                            "span_id": str(span.span_id),
                            "parent_span_id": str(parent_id),
                        },
                    )
                depth = 0
            else:
                depth = resolve_depth(parent) + 1
        visiting.remove(key)
        depths[key] = depth
        return depth

    frames: list[TraceReplayFrame] = []
    for span in sorted(
        span_list,
        key=lambda item: (str(item.trace_id), item.start, str(item.span_id)),
    ):
        frames.append(
            TraceReplayFrame(
                span=span,
                trace_id=span.trace_id,
                span_id=span.span_id,
                parent_span_id=span.parent_span_id,
                name=span.name,
                status=span.status,
                depth=resolve_depth(span),
            )
        )
    return tuple(frames)


def _key(span: TraceSpan) -> _SpanKey:
    return (str(span.trace_id), str(span.span_id))


def _raise_replay_error(
    *,
    route: str,
    evidence: dict[str, str | int | float | bool],
) -> None:
    raise TraceReplayError(
        Error(
            code=Errs.TRACE_REPLAY_FAILED,
            source="mutsukibot.testing.trace_replay",
            route=route,
            evidence=evidence,
        )
    )


__all__ = ["TraceReplayError", "TraceReplayFrame", "replay_trace_spans"]
