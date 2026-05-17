"""Testing utilities for MutsukiBot's own contract and lint suites."""

from mutsukibot.testing.benchmark import DispatchInvokeStats, measure_dispatcher_invoke
from mutsukibot.testing.plugin_lint import PluginIoFieldViolation, lint_plugin_io_fields
from mutsukibot.testing.trace_replay import (
    TraceReplayError,
    TraceReplayFrame,
    replay_trace_spans,
)

__all__ = [
    "DispatchInvokeStats",
    "PluginIoFieldViolation",
    "TraceReplayError",
    "TraceReplayFrame",
    "lint_plugin_io_fields",
    "measure_dispatcher_invoke",
    "replay_trace_spans",
]
