"""Runner protocol and host helpers."""

from mutsuki_runner_kit.runners.async_adapter import (
    AsyncRunnerContext,
    RuntimeClient,
    TaskAwaitRunnerAdapter,
    TaskCallAwaitable,
)
from mutsuki_runner_kit.runners.scalar import (
    ScalarBatchAdapter,
    ScalarPythonRunner,
    single_entry_batch,
)

__all__ = [
    "AsyncRunnerContext",
    "RuntimeClient",
    "ScalarBatchAdapter",
    "ScalarPythonRunner",
    "TaskAwaitRunnerAdapter",
    "TaskCallAwaitable",
    "single_entry_batch",
]
