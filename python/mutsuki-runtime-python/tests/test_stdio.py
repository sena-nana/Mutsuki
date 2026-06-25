from __future__ import annotations

import pytest

from mutsuki_runtime_python.contracts.codec import to_json_dict
from mutsuki_runtime_python.contracts.runner import RunnerContext
from mutsuki_runtime_python.contracts.task import Task
from mutsuki_runtime_python.runners.host import PythonRunnerHost
from mutsuki_runtime_python.testing.runners import EchoRunner, echo_descriptor
from mutsuki_runtime_python.transport.stdio_jsonl import StdioJsonlRunnerServer


@pytest.mark.asyncio
async def test_stdio_runner_step_dispatches_to_host() -> None:
    host = PythonRunnerHost()
    host.register_runner(EchoRunner(echo_descriptor()))
    server = StdioJsonlRunnerServer(host)

    response = await server.handle_request(
        {
            "id": "req-1",
            "method": "runner.step",
            "params": {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(RunnerContext(registry_generation=1, current_step=1)),
                "tasks": [to_json_dict(Task.new("task-1", "raw.input"))],
            },
        }
    )

    assert response["ok"] is True
    assert response["result"][0]["task_id"] == "task-1"  # type: ignore[index]


@pytest.mark.asyncio
async def test_stdio_unknown_runner_returns_structured_error() -> None:
    server = StdioJsonlRunnerServer(PythonRunnerHost())

    response = await server.handle_request(
        {
            "id": "req-1",
            "method": "runner.cancel",
            "params": {"runner_id": "missing", "invocation_id": "inv-1"},
        }
    )

    assert response["ok"] is False
    assert response["error"]["code"] == "runner.not_found"  # type: ignore[index]
