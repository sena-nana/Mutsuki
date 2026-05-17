"""Reusable contract-test assertions.

这些 helper 不驱动业务逻辑，只把常见契约断言收敛到一个稳定入口，方便
core、reference plugin 与未来第三方实现复用同一套检查。
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any

from mutsukibot.contracts.event import TraceSpan
from mutsukibot.testing.trace_replay import TraceReplayFrame, replay_trace_spans


@dataclass(frozen=True, slots=True)
class CrossAgentTraceLink:
    """跨 Agent trace 中 caller span 与 target span 的闭合父子关系。"""

    caller: TraceReplayFrame
    target: TraceReplayFrame


def assert_trace_tree_closed(
    spans: Iterable[TraceSpan],
) -> tuple[TraceReplayFrame, ...]:
    """Assert that all parent spans are present and return deterministic frames."""
    return replay_trace_spans(spans, require_known_parents=True)


def assert_cross_agent_trace_chain(
    spans: Iterable[TraceSpan],
    *,
    caller_name: str = "dispatch.invoke_in_agent",
    target_name: str = "dispatch.invoke",
) -> CrossAgentTraceLink:
    """Assert that a cross-Agent call keeps one closed parent-child trace chain."""
    frames = replay_trace_spans(spans)
    caller = _find_frame(frames, caller_name)
    target = _find_frame(frames, target_name)

    if target.trace_id != caller.trace_id or target.parent_span_id != caller.span_id:
        raise AssertionError(
            "trace chain is not closed: "
            f"{target_name!r} parent={target.parent_span_id!r}, "
            f"expected {caller.span_id!r}"
        )
    return CrossAgentTraceLink(caller=caller, target=target)


def assert_dispatcher_clean(agent: Any) -> None:
    """Assert that plugin unload left no dispatcher Operation / Source residue."""
    operations = tuple(agent.dispatch.list_operations())
    sources = tuple(agent.dispatch.list_sources())
    if operations or sources:
        raise AssertionError(
            "dispatcher has residual registrations: "
            f"operations={operations!r}, sources={sources!r}"
        )


def _find_frame(
    frames: tuple[TraceReplayFrame, ...],
    name: str,
) -> TraceReplayFrame:
    for frame in frames:
        if frame.name == name:
            return frame
    raise AssertionError(f"trace span {name!r} was not recorded")


__all__ = [
    "CrossAgentTraceLink",
    "assert_cross_agent_trace_chain",
    "assert_dispatcher_clean",
    "assert_trace_tree_closed",
]
