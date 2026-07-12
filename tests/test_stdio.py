from __future__ import annotations

from dataclasses import replace

import pytest

from mutsuki_runner_kit.contracts.codec import to_json_dict
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.batches import runner_context, single_test_batch
from mutsuki_runner_kit.testing.fake_resource_provider import FakeResourceProvider
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor
from mutsuki_runner_kit.transport.stdio_jsonl import StdioJsonlBridge


class CaptureContextRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        super().__init__(descriptor)
        self.contexts: list[RunnerContext] = []

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        self.contexts.append(ctx)
        return await super().run_one(ctx, task)


@pytest.mark.asyncio
async def test_stdio_runner_run_batch_dispatches_to_host() -> None:
    backend = PythonRunnerBackend()
    runner = CaptureContextRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioJsonlBridge(backend)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    ctx = replace(runner_context(deadline_tick=3), invocation_id="task-1", cancel_token="task-1")
    batch = single_test_batch(task)

    response = await bridge.handle_request(
        {
            "id": "req-1",
            "method": "runner.run_batch",
            "params": {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        }
    )

    assert response["ok"] is True
    assert response["result"]["batch_id"] == "batch-test"  # type: ignore[index]
    assert response["result"]["results"][0]["task_id"] == "task-1"  # type: ignore[index]
    assert response["result"]["results"][0]["result"]["task_await"] is None  # type: ignore[index]
    assert runner.contexts[0].invocation_id == "task-1"
    assert runner.contexts[0].cancel_token == "task-1"
    assert runner.contexts[0].deadline_tick == 3
    assert runner.contexts[0].task_lease_ids == ("task-lease-test",)


@pytest.mark.asyncio
async def test_stdio_unknown_runner_returns_structured_error() -> None:
    bridge = StdioJsonlBridge(PythonRunnerBackend())

    response = await bridge.handle_request(
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
    backend = PythonRunnerBackend()
    runner = EchoRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioJsonlBridge(backend)

    cancel_response = await bridge.handle_request(
        {
            "id": "req-1",
            "method": "runner.cancel",
            "params": {"runner_id": "echo.runner", "invocation_id": "inv-1"},
        }
    )
    dispose_response = await bridge.handle_request(
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
    manager = FakeResourceProvider()
    text = manager.create_blob_resource("text.v1", b"hello")
    capability = manager.create_capability_resource("db_pool", "db.pool.v1")
    command = manager.command_plan(capability, "query", {"sql": "select 1"}, "query:1")
    bridge = StdioJsonlBridge(PythonRunnerBackend(), manager)

    export_response = await bridge.handle_request(
        {
            "id": "req-1",
            "method": "resource.export",
            "params": {"plan": to_json_dict(manager.export_plan(text, "inline_utf8"))},
        }
    )
    command_response = await bridge.handle_request(
        {
            "id": "req-2",
            "method": "resource.command",
            "params": {"plan": to_json_dict(command)},
        }
    )
    batch_response = await bridge.handle_request(
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
    saga_response = await bridge.handle_request(
        {
            "id": "req-4",
            "method": "resource.saga",
            "params": {"saga": to_json_dict(manager.saga_plan("saga:1", (command,), (command,)))},
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
async def test_stdio_runner_run_batch_returns_structured_lease_mismatch_error() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(EchoRunner(echo_descriptor()))
    bridge = StdioJsonlBridge(backend)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-task")
    batch = single_test_batch(task, lease_id="task-lease-task")
    ctx = runner_context(lease_ids=("task-lease-ctx",))

    response = await bridge.handle_request(
        {
            "id": "req-1",
            "method": "runner.run_batch",
            "params": {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        }
    )

    assert response["ok"] is False
    assert response["error"]["code"] == "task.claim_conflict"  # type: ignore[index]


@pytest.mark.asyncio
async def test_stdio_unknown_method_and_runner_step_are_rejected() -> None:
    bridge = StdioJsonlBridge(PythonRunnerBackend())

    unknown = await bridge.handle_request(
        {"id": "req-1", "method": "runner.step", "params": {"runner_id": "echo.runner"}}
    )

    assert unknown["ok"] is False
    assert unknown["error"]["code"] == "runtime.host_failed"  # type: ignore[index]
    assert unknown["error"]["evidence"]["reason"] == "unknown_method"  # type: ignore[index]

    missing_resource = await bridge.handle_request(
        {"id": "req-2", "method": "resource.export", "params": {}}
    )
    assert missing_resource["ok"] is False
    assert missing_resource["error"]["evidence"]["reason"] == "resource_handler_missing"  # type: ignore[index]
