"""Testing utilities for MutsukiBot's own contract and lint suites."""

from mutsukibot.testing.benchmark import DispatchInvokeStats, measure_dispatcher_invoke
from mutsukibot.testing.contract_kit import (
    CrossAgentTraceLink,
    assert_cross_agent_trace_chain,
    assert_dispatcher_clean,
    assert_trace_tree_closed,
)
from mutsukibot.testing.plugin_lint import PluginIoFieldViolation, lint_plugin_io_fields
from mutsukibot.testing.trace_replay import (
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
