"""Testing utilities for Mutsuki's own contract and lint suites."""

from mutsuki.testing.benchmark import DispatchInvokeStats, measure_dispatcher_invoke
from mutsuki.testing.contract_kit import (
    CrossAgentTraceLink,
    assert_cross_agent_trace_chain,
    assert_dispatcher_clean,
    assert_trace_tree_closed,
)
from mutsuki.testing.plugin_lint import PluginIoFieldViolation, lint_plugin_io_fields
from mutsuki.testing.trace_replay import (
    TraceReplayError,
    TraceReplayFrame,
    replay_trace_spans,
)

__all__ = [
    "CrossAgentTraceLink",
    "DispatchInvokeStats",
    "PluginIoFieldViolation",
    "TraceReplayError",
    "TraceReplayFrame",
    "assert_cross_agent_trace_chain",
    "assert_dispatcher_clean",
    "assert_trace_tree_closed",
    "lint_plugin_io_fields",
    "measure_dispatcher_invoke",
    "replay_trace_spans",
]
