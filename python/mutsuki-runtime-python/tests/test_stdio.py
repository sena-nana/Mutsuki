from __future__ import annotations

from dataclasses import replace

import pytest

from mutsuki_runtime_python.contracts.codec import to_json_dict
from mutsuki_runtime_python.contracts.runner import RunnerContext
from mutsuki_runtime_python.contracts.task import Task
from mutsuki_runtime_python.resources.manager import PythonResourceManager
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
                "ctx": to_json_dict(
                    RunnerContext(
                        registry_generation=1,
                        current_step=1,
                        executor_id="executor:test",
                        task_lease_id="task-lease-test",
                    )
                ),
                "tasks": [
                    to_json_dict(
                        replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
                    )
                ],
            },
        }
    )

    assert response["ok"] is True
    assert response["result"][0]["task_id"] == "task-1"  # type: ignore[index]
    assert response["result"][0]["task_await"] is None  # type: ignore[index]


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


@pytest.mark.asyncio
async def test_stdio_cancel_and_dispose_dispatch_to_host_management_channel() -> None:
    host = PythonRunnerHost()
    runner = EchoRunner(echo_descriptor())
    host.register_runner(runner)
    server = StdioJsonlRunnerServer(host)

    cancel_response = await server.handle_request(
        {
            "id": "req-1",
            "method": "runner.cancel",
            "params": {"runner_id": "echo.runner", "invocation_id": "inv-1"},
        }
    )
    dispose_response = await server.handle_request(
        {
            "id": "req-2",
            "method": "runner.dispose",
            "params": {"runner_id": "echo.runner"},
        }
    )

    assert cancel_response == {"id": "req-1", "ok": True, "result": None}
    assert dispose_response == {"id": "req-2", "ok": True, "result": None}
    assert runner.cancelled == ["inv-1"]
    assert runner.disposed is True


@pytest.mark.asyncio
async def test_stdio_resource_plan_methods_dispatch_to_resource_manager() -> None:
    manager = PythonResourceManager()
    text = manager.create_blob_resource("text.v1", b"hello")
    capability = manager.create_capability_resource("db_pool", "db.pool.v1")
    command = manager.command_plan(capability, "query", {"sql": "select 1"}, "query:1")
    server = StdioJsonlRunnerServer(PythonRunnerHost(), manager)

    export_response = await server.handle_request(
        {
            "id": "req-1",
            "method": "resource.export",
            "params": {"plan": to_json_dict(manager.export_plan(text, "inline_utf8"))},
        }
    )
    command_response = await server.handle_request(
        {
            "id": "req-2",
            "method": "resource.command",
            "params": {"plan": to_json_dict(command)},
        }
    )
    batch_response = await server.handle_request(
        {
            "id": "req-3",
            "method": "resource.command_batch",
            "params": {
                "batch": to_json_dict(
                    manager.command_batch("batch:1", (command,), rollback_guarantee=False)
                )
            },
        }
    )
    saga_response = await server.handle_request(
        {
            "id": "req-4",
            "method": "resource.saga",
            "params": {
                "saga": to_json_dict(manager.saga_plan("saga:1", (command,), (command,)))
            },
        }
    )

    assert export_response["ok"] is True
    assert export_response["result"]["status"] == "exported"  # type: ignore[index]
    assert export_response["result"]["output"] == "hello"  # type: ignore[index]
    assert command_response["ok"] is True
    assert command_response["result"]["status"] == "commanded"  # type: ignore[index]
    assert len(batch_response["result"]) == 1  # type: ignore[arg-type]
    assert len(saga_response["result"]) == 1  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_stdio_runner_step_returns_structured_lease_mismatch_error() -> None:
    host = PythonRunnerHost()
    host.register_runner(EchoRunner(echo_descriptor()))
    server = StdioJsonlRunnerServer(host)

    response = await server.handle_request(
        {
            "id": "req-1",
            "method": "runner.step",
            "params": {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(
                    RunnerContext(
                        registry_generation=1,
                        current_step=1,
                        executor_id="executor:test",
                        task_lease_id="task-lease-ctx",
                    )
                ),
                "tasks": [
                    to_json_dict(
                        replace(Task.new("task-1", "raw.input"), lease_id="task-lease-task")
                    )
                ],
            },
        }
    )

    assert response["ok"] is False
    assert response["error"]["code"] == "task.claim_conflict"  # type: ignore[index]
