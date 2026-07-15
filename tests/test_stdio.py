from __future__ import annotations

import asyncio
import io
import json
import sys
import time
from dataclasses import replace
from typing import cast

import pytest

from mutsuki_runner_kit.contracts.codec import JsonDict, JsonValue, to_json_dict
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.batches import runner_context, single_test_batch
from mutsuki_runner_kit.testing.fake_resource_provider import FakeResourceProvider
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor
from mutsuki_runner_kit.transport.stdio_jsonl import StdioJsonlBridge
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.jsonl import decode_jsonl_request, encode_jsonl_request
from mutsuki_runner_kit.wire.protocol import (
    DEBUG_JSONL_CODEC_ID,
    DEFAULT_WIRE_LIMITS,
    ProtocolHello,
    WireLimits,
    WireProtocolFailure,
)


class CaptureContextRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        super().__init__(descriptor)
        self.contexts: list[RunnerContext] = []

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        self.contexts.append(ctx)
        return await super().run_one(ctx, task)


class BlockingCancelRunner(EchoRunner):
    def __init__(self, descriptor: RunnerDescriptor) -> None:
        super().__init__(descriptor)
        self.started = asyncio.Event()
        self.cancelled_event = asyncio.Event()

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        self.started.set()
        await self.cancelled_event.wait()
        await asyncio.sleep(0.01)
        return await super().run_one(ctx, task)

    async def cancel(self, invocation_id: str) -> None:
        await super().cancel(invocation_id)
        self.cancelled_event.set()


class PrintingRunner(EchoRunner):
    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        sys.stdout.write("runner diagnostic\n")
        return await super().run_one(ctx, task)


class BlockingDisposeRunner(BlockingCancelRunner):
    async def dispose(self) -> None:
        await super().dispose()
        self.cancelled_event.set()


@pytest.mark.asyncio
async def test_stdio_runner_run_batch_dispatches_typed_request() -> None:
    backend = PythonRunnerBackend()
    runner = CaptureContextRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioJsonlBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    ctx = replace(runner_context(deadline_tick=3), invocation_id="task-1", cancel_token="task-1")
    batch = single_test_batch(task)

    response = await bridge.handle_request(
        request_mapping(
            2,
            Opcode.RUNNER_RUN_BATCH,
            {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        )
    )

    assert response["ok"] is True
    result = cast(dict[str, JsonValue], response["result"])
    assert result["batch_id"] == "batch-test"
    assert runner.contexts[0].executor_id == "executor:test"
    assert runner.contexts[0].deadline_tick == 3
    assert runner.contexts[0].task_lease_ids == ("task-lease-test",)


@pytest.mark.asyncio
async def test_stdio_breaking_version_fails_during_initialize() -> None:
    bridge = StdioJsonlBridge(PythonRunnerBackend())
    request = request_mapping(
        1,
        Opcode.PLUGIN_INITIALIZE,
        {"hello": ProtocolHello.for_codec(DEBUG_JSONL_CODEC_ID).to_dict()},
    )
    protocol = cast(dict[str, object], request["protocol"])
    protocol["major"] = 2

    response = await bridge.handle_request(request)

    assert response["ok"] is False
    error = cast(dict[str, JsonValue], response["error"])
    assert error["route"] == "wire.version_mismatch"


@pytest.mark.asyncio
async def test_stdio_unknown_runner_returns_structured_error() -> None:
    bridge = StdioJsonlBridge(PythonRunnerBackend())
    await initialize(bridge)

    response = await bridge.handle_request(
        request_mapping(
            2,
            Opcode.RUNNER_CANCEL,
            {"runner_id": "missing", "invocation_id": "inv-1"},
        )
    )

    assert response["ok"] is False
    error = cast(dict[str, JsonValue], response["error"])
    assert error["code"] == "runner.not_found"


@pytest.mark.asyncio
async def test_stdio_cancel_and_dispose_dispatch_to_management_channel() -> None:
    backend = PythonRunnerBackend()
    runner = EchoRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioJsonlBridge(backend)
    await initialize(bridge)

    cancel_response = await bridge.handle_request(
        request_mapping(
            2,
            Opcode.RUNNER_CANCEL,
            {"runner_id": "echo.runner", "invocation_id": "inv-1"},
        )
    )
    dispose_response = await bridge.handle_request(
        request_mapping(3, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"})
    )

    assert cancel_response["ok"] is True
    assert cancel_response["result"] is None
    assert dispose_response["ok"] is True
    assert runner.cancelled == ["inv-1"]
    assert runner.disposed is True


@pytest.mark.asyncio
async def test_stdio_resource_plan_methods_use_injected_handler() -> None:
    manager = FakeResourceProvider()
    text = manager.create_blob_resource("text.v1", b"hello")
    capability = manager.create_capability_resource("db_pool", "db.pool.v1")
    command = manager.command_plan(capability, "query", {"sql": "select 1"}, "query:1")
    bridge = StdioJsonlBridge(PythonRunnerBackend(), manager)
    await initialize(bridge)

    export_response = await bridge.handle_request(
        request_mapping(
            2,
            Opcode.RESOURCE_EXPORT,
            {"provider_id": None, "plan": to_json_dict(manager.export_plan(text, "inline_utf8"))},
        )
    )
    command_response = await bridge.handle_request(
        request_mapping(
            3,
            Opcode.RESOURCE_COMMAND,
            {"provider_id": None, "plan": to_json_dict(command)},
        )
    )
    batch_response = await bridge.handle_request(
        request_mapping(
            4,
            Opcode.RESOURCE_COMMAND_BATCH,
            {
                "provider_id": None,
                "batch": to_json_dict(
                    manager.command_batch("batch:1", (command,), rollback_guarantee=False)
                ),
            },
        )
    )
    saga_response = await bridge.handle_request(
        request_mapping(
            5,
            Opcode.RESOURCE_SAGA,
            {
                "provider_id": None,
                "saga": to_json_dict(
                    manager.saga_plan("saga:1", (command,), (command,))
                ),
            },
        )
    )

    assert export_response["ok"] is True
    assert cast(dict[str, JsonValue], export_response["result"])["output"] == "hello"
    assert command_response["ok"] is True
    assert len(cast(list[JsonValue], batch_response["result"])) == 1
    assert len(cast(list[JsonValue], saga_response["result"])) == 1


@pytest.mark.asyncio
async def test_stdio_runner_run_batch_returns_structured_lease_mismatch() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(EchoRunner(echo_descriptor()))
    bridge = StdioJsonlBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-task")
    batch = single_test_batch(task, lease_id="task-lease-task")
    ctx = runner_context(lease_ids=("task-lease-ctx",))

    response = await bridge.handle_request(
        request_mapping(
            2,
            Opcode.RUNNER_RUN_BATCH,
            {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        )
    )

    assert response["ok"] is False
    error = cast(dict[str, JsonValue], response["error"])
    assert error["code"] == "task.claim_conflict"


@pytest.mark.asyncio
async def test_stdio_method_mismatch_and_missing_resource_handler_fail_loud() -> None:
    bridge = StdioJsonlBridge(PythonRunnerBackend())
    await initialize(bridge)
    mismatched = request_mapping(
        2,
        Opcode.RUNNER_CANCEL,
        {"runner_id": "echo.runner", "invocation_id": "inv-1"},
    )
    mismatched["method"] = "runner.dispose"

    unknown = await bridge.handle_request(mismatched)
    missing_resource = await bridge.handle_request(
        request_mapping(
            3,
            Opcode.RESOURCE_EXPORT,
            {"provider_id": None, "plan": {}},
        )
    )

    assert unknown["ok"] is False
    error = cast(dict[str, JsonValue], unknown["error"])
    assert error["route"] == "wire.method_mismatch"
    assert missing_resource["ok"] is False


@pytest.mark.asyncio
async def test_stdio_run_batch_does_not_block_cancel_or_response_correlation() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingCancelRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioJsonlBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    batch = single_test_batch(task)
    run = encode_jsonl_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(batch),
        },
    )
    cancel = encode_jsonl_request(
        3,
        Opcode.RUNNER_CANCEL,
        {"runner_id": "echo.runner", "invocation_id": "invocation:test"},
    )
    output = io.StringIO()
    started = time.perf_counter()

    await asyncio.wait_for(
        bridge.serve(io.StringIO((run + cancel).decode()), output), timeout=1
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    responses = [json.loads(line) for line in output.getvalue().splitlines()]

    assert runner.started.is_set()
    assert runner.cancelled == ["invocation:test"]
    assert {response["request_id"] for response in responses} == {2, 3}
    assert [response["request_id"] for response in responses] == [3, 2]
    assert elapsed_ms < 500


@pytest.mark.asyncio
async def test_stdio_run_batch_does_not_block_dispose() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingDisposeRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioJsonlBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    run = encode_jsonl_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(single_test_batch(task)),
        },
    )
    dispose = encode_jsonl_request(
        3, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"}
    )
    output = io.StringIO()

    await asyncio.wait_for(
        bridge.serve(io.StringIO((run + dispose).decode()), output), timeout=1
    )

    assert runner.disposed is True
    assert {json.loads(line)["request_id"] for line in output.getvalue().splitlines()} == {
        2,
        3,
    }


@pytest.mark.asyncio
async def test_management_capacity_remains_available_when_work_is_saturated() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingCancelRunner(echo_descriptor())
    backend.register_runner(runner)
    limits = WireLimits(
        max_frame_bytes=DEFAULT_WIRE_LIMITS.max_frame_bytes,
        max_payload_bytes=DEFAULT_WIRE_LIMITS.max_payload_bytes,
        max_jsonl_line_bytes=DEFAULT_WIRE_LIMITS.max_jsonl_line_bytes,
        max_inline_resource_bytes=DEFAULT_WIRE_LIMITS.max_inline_resource_bytes,
        max_in_flight_requests=2,
        management_reserved_requests=1,
    )
    bridge = StdioJsonlBridge(backend, limits=limits)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    payload = {
        "runner_id": "echo.runner",
        "ctx": to_json_dict(runner_context()),
        "batch": to_json_dict(single_test_batch(task)),
    }
    input_bytes = b"".join(
        (
            encode_jsonl_request(2, Opcode.RUNNER_RUN_BATCH, payload),
            encode_jsonl_request(3, Opcode.RUNNER_RUN_BATCH, payload),
            encode_jsonl_request(
                4,
                Opcode.RUNNER_CANCEL,
                {"runner_id": "echo.runner", "invocation_id": "invocation:test"},
            ),
        )
    )
    output = io.StringIO()

    await asyncio.wait_for(
        bridge.serve(io.StringIO(input_bytes.decode()), output), timeout=1
    )
    responses = {response["request_id"]: response for response in map(
        json.loads, output.getvalue().splitlines()
    )}

    assert responses[3]["ok"] is False
    assert responses[3]["error"]["route"] == "wire.pending_exhausted"
    assert responses[4]["ok"] is True


@pytest.mark.asyncio
async def test_protocol_stdout_is_not_polluted_by_runner_prints() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(PrintingRunner(echo_descriptor()))
    diagnostics = io.StringIO()
    bridge = StdioJsonlBridge(backend, diagnostics=diagnostics)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    run = encode_jsonl_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(single_test_batch(task)),
        },
    )
    protocol_output = io.StringIO()
    await bridge.serve(io.StringIO(run.decode()), protocol_output)

    assert "runner diagnostic" in diagnostics.getvalue()
    assert "runner diagnostic" not in protocol_output.getvalue()
    assert json.loads(protocol_output.getvalue())["ok"] is True


def test_jsonl_oversized_frame_is_rejected_without_unbounded_read() -> None:
    limits = WireLimits(
        max_frame_bytes=128,
        max_payload_bytes=64,
        max_jsonl_line_bytes=128,
        max_inline_resource_bytes=32,
        max_in_flight_requests=2,
        management_reserved_requests=1,
    )

    with pytest.raises(WireProtocolFailure, match=r"wire\.frame_oversized"):
        decode_jsonl_request(b"x" * 129, limits)


@pytest.mark.asyncio
async def test_malformed_jsonl_is_diagnosed_without_stopping_later_frames() -> None:
    backend = PythonRunnerBackend()
    runner = EchoRunner(echo_descriptor())
    backend.register_runner(runner)
    diagnostics = io.StringIO()
    bridge = StdioJsonlBridge(backend, diagnostics=diagnostics)
    await initialize(bridge)
    dispose = encode_jsonl_request(
        2, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"}
    )
    output = io.StringIO()

    await bridge.serve(io.StringIO("{malformed}\n" + dispose.decode()), output)

    assert "malformed uncorrelated JSONL frame" in diagnostics.getvalue()
    response = json.loads(output.getvalue())
    assert response["request_id"] == 2
    assert response["ok"] is True


@pytest.mark.asyncio
async def test_jsonl_eof_has_bounded_shutdown_for_running_work() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingCancelRunner(echo_descriptor())
    backend.register_runner(runner)
    diagnostics = io.StringIO()
    bridge = StdioJsonlBridge(
        backend, diagnostics=diagnostics, shutdown_timeout=0.01
    )
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
    run = encode_jsonl_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(single_test_batch(task)),
        },
    )

    await asyncio.wait_for(
        bridge.serve(io.StringIO(run.decode()), io.StringIO()), timeout=0.5
    )

    assert runner.started.is_set()
    assert "wire.shutdown_timeout" in diagnostics.getvalue()


async def initialize(bridge: StdioJsonlBridge) -> None:
    response = await bridge.handle_request(
        request_mapping(
            1,
            Opcode.PLUGIN_INITIALIZE,
            {"hello": ProtocolHello.for_codec(DEBUG_JSONL_CODEC_ID).to_dict()},
        )
    )
    assert response["ok"] is True


def request_mapping(request_id: int, opcode: Opcode, payload: JsonDict) -> dict[str, object]:
    return cast(
        dict[str, object],
        json.loads(encode_jsonl_request(request_id, opcode, payload)),
    )
