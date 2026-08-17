from __future__ import annotations

import asyncio
import io
import struct
import sys
import time
from dataclasses import replace
from typing import cast

import pytest

from mutsuki_runner_kit.contracts.codec import JsonValue, to_json_dict
from mutsuki_runner_kit.contracts.ids import TaskLeaseId
from mutsuki_runner_kit.contracts.runner import RunnerContext, RunnerDescriptor, RunnerResult
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.batches import runner_context, single_test_batch
from mutsuki_runner_kit.testing.fake_resource_provider import FakeResourceProvider
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor
from mutsuki_runner_kit.transport.stdio_binary import StdioBinaryBridge
from mutsuki_runner_kit.wire.binary import (
    binary_response_payload,
    encode_binary_request,
    read_binary_request,
)
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.protocol import (
    BINARY_CODEC_ID,
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
    bridge = StdioBinaryBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id=TaskLeaseId("task-lease-test"))
    ctx = replace(runner_context(deadline_tick=3), invocation_id="task-1", cancel_token="task-1")
    batch = single_test_batch(task)

    responses = await send_frames(
        bridge,
        encode_binary_request(
            2,
            Opcode.RUNNER_RUN_BATCH,
            {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        ),
    )

    request_id, opcode, is_error, payload = responses[0]
    result = cast(dict[str, JsonValue], payload)
    assert (request_id, opcode, is_error) == (2, Opcode.RUNNER_RUN_BATCH, False)
    assert result["batch_id"] == "batch-test"
    assert runner.contexts[0].executor_id == "executor:test"
    assert runner.contexts[0].deadline_tick == 3
    assert runner.contexts[0].task_lease_ids == ("task-lease-test",)


@pytest.mark.asyncio
async def test_stdio_breaking_version_fails_during_initialize() -> None:
    bridge = StdioBinaryBridge(PythonRunnerBackend())
    hello = ProtocolHello.for_codec(BINARY_CODEC_ID).to_dict()
    protocol = cast(dict[str, object], hello["protocol"])
    protocol["major"] = 2

    responses = await send_frames(
        bridge,
        encode_binary_request(1, Opcode.PLUGIN_INITIALIZE, {"hello": hello}),
    )

    _, _, is_error, payload = responses[0]
    error = cast(dict[str, JsonValue], payload)
    assert is_error is True
    assert error["route"] == "wire.version_mismatch"


@pytest.mark.asyncio
async def test_stdio_unknown_runner_returns_structured_error() -> None:
    bridge = StdioBinaryBridge(PythonRunnerBackend())
    await initialize(bridge)

    responses = await send_frames(
        bridge,
        encode_binary_request(
            2,
            Opcode.RUNNER_CANCEL,
            {"runner_id": "missing", "invocation_id": "inv-1"},
        ),
    )

    _, _, is_error, payload = responses[0]
    error = cast(dict[str, JsonValue], payload)
    assert is_error is True
    assert error["code"] == "runner.not_found"


@pytest.mark.asyncio
async def test_stdio_cancel_and_dispose_dispatch_to_management_channel() -> None:
    backend = PythonRunnerBackend()
    runner = EchoRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioBinaryBridge(backend)
    await initialize(bridge)

    responses = await send_frames(
        bridge,
        encode_binary_request(
            2,
            Opcode.RUNNER_CANCEL,
            {"runner_id": "echo.runner", "invocation_id": "inv-1"},
        ),
        encode_binary_request(3, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"}),
    )

    assert [(item[0], item[1], item[2]) for item in responses] == [
        (2, Opcode.RUNNER_CANCEL, False),
        (3, Opcode.RUNNER_DISPOSE, False),
    ]
    assert runner.cancelled == ["inv-1"]
    assert runner.disposed is True


@pytest.mark.asyncio
async def test_stdio_resource_plan_methods_use_injected_handler() -> None:
    manager = FakeResourceProvider()
    text = manager.create_blob_resource("text.v1", b"hello")
    capability = manager.create_capability_resource("db_pool", "db.pool.v1")
    command = manager.command_plan(capability, "query", {"sql": "select 1"}, "query:1")
    bridge = StdioBinaryBridge(PythonRunnerBackend(), manager)
    await initialize(bridge)

    responses = await send_frames(
        bridge,
        encode_binary_request(
            2,
            Opcode.RESOURCE_EXPORT,
            {"provider_id": None, "plan": to_json_dict(manager.export_plan(text, "inline_utf8"))},
        ),
        encode_binary_request(
            3,
            Opcode.RESOURCE_COMMAND,
            {"provider_id": None, "plan": to_json_dict(command)},
        ),
        encode_binary_request(
            4,
            Opcode.RESOURCE_COMMAND_BATCH,
            {
                "provider_id": None,
                "batch": to_json_dict(
                    manager.command_batch("batch:1", (command,), rollback_guarantee=False)
                ),
            },
        ),
        encode_binary_request(
            5,
            Opcode.RESOURCE_SAGA,
            {
                "provider_id": None,
                "saga": to_json_dict(manager.saga_plan("saga:1", (command,), (command,))),
            },
        ),
    )

    by_id = {request_id: (is_error, payload) for request_id, _, is_error, payload in responses}
    assert by_id[2][0] is False
    assert cast(dict[str, JsonValue], by_id[2][1])["output"] == "hello"
    assert by_id[3][0] is False
    assert len(cast(list[JsonValue], by_id[4][1])) == 1
    assert len(cast(list[JsonValue], by_id[5][1])) == 1


@pytest.mark.asyncio
async def test_stdio_runner_run_batch_returns_structured_lease_mismatch() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(EchoRunner(echo_descriptor()))
    bridge = StdioBinaryBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id=TaskLeaseId("task-lease-task"))
    batch = single_test_batch(task, lease_id="task-lease-task")
    ctx = runner_context(lease_ids=("task-lease-ctx",))

    responses = await send_frames(
        bridge,
        encode_binary_request(
            2,
            Opcode.RUNNER_RUN_BATCH,
            {
                "runner_id": "echo.runner",
                "ctx": to_json_dict(ctx),
                "batch": to_json_dict(batch),
            },
        ),
    )

    _, _, is_error, payload = responses[0]
    error = cast(dict[str, JsonValue], payload)
    assert is_error is True
    assert error["code"] == "task.claim_conflict"


@pytest.mark.asyncio
async def test_missing_resource_handler_fails_loud() -> None:
    bridge = StdioBinaryBridge(PythonRunnerBackend())
    await initialize(bridge)
    manager = FakeResourceProvider()
    text = manager.create_blob_resource("text/plain", b"hello")

    responses = await send_frames(
        bridge,
        encode_binary_request(
            3,
            Opcode.RESOURCE_EXPORT,
            {"provider_id": None, "plan": to_json_dict(manager.export_plan(text, "inline_utf8"))},
        ),
    )

    assert responses[0][2] is True


@pytest.mark.asyncio
async def test_stdio_run_batch_does_not_block_cancel_or_response_correlation() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingCancelRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioBinaryBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id=TaskLeaseId("task-lease-test"))
    batch = single_test_batch(task)
    run = encode_binary_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(batch),
        },
    )
    cancel = encode_binary_request(
        3,
        Opcode.RUNNER_CANCEL,
        {"runner_id": "echo.runner", "invocation_id": "invocation:test"},
    )
    started = time.perf_counter()

    responses = await asyncio.wait_for(send_frames(bridge, run, cancel), timeout=1)
    elapsed_ms = (time.perf_counter() - started) * 1000

    assert runner.started.is_set()
    assert runner.cancelled == ["invocation:test"]
    assert {response[0] for response in responses} == {2, 3}
    assert [response[0] for response in responses] == [3, 2]
    assert elapsed_ms < 500


@pytest.mark.asyncio
async def test_stdio_run_batch_does_not_block_dispose() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingDisposeRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioBinaryBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id=TaskLeaseId("task-lease-test"))
    run = encode_binary_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(single_test_batch(task)),
        },
    )
    dispose = encode_binary_request(3, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"})

    responses = await asyncio.wait_for(send_frames(bridge, run, dispose), timeout=1)

    assert runner.disposed is True
    assert {response[0] for response in responses} == {2, 3}


@pytest.mark.asyncio
async def test_management_capacity_remains_available_when_work_is_saturated() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingCancelRunner(echo_descriptor())
    backend.register_runner(runner)
    limits = WireLimits(
        max_frame_bytes=DEFAULT_WIRE_LIMITS.max_frame_bytes,
        max_payload_bytes=DEFAULT_WIRE_LIMITS.max_payload_bytes,
        max_inline_resource_bytes=DEFAULT_WIRE_LIMITS.max_inline_resource_bytes,
        max_in_flight_requests=2,
        management_reserved_requests=1,
    )
    bridge = StdioBinaryBridge(backend, limits=limits)
    await initialize(bridge, limits)
    task = replace(Task.new("task-1", "raw.input"), lease_id=TaskLeaseId("task-lease-test"))
    payload = {
        "runner_id": "echo.runner",
        "ctx": to_json_dict(runner_context()),
        "batch": to_json_dict(single_test_batch(task)),
    }

    responses = await asyncio.wait_for(
        send_frames(
            bridge,
            encode_binary_request(2, Opcode.RUNNER_RUN_BATCH, payload, limits),
            encode_binary_request(3, Opcode.RUNNER_RUN_BATCH, payload, limits),
            encode_binary_request(
                4,
                Opcode.RUNNER_CANCEL,
                {"runner_id": "echo.runner", "invocation_id": "invocation:test"},
                limits,
            ),
        ),
        timeout=1,
    )
    by_id = {request_id: payload for request_id, _, _, payload in responses}

    assert cast(dict[str, JsonValue], by_id[3])["route"] == "wire.pending_exhausted"
    assert any(response[0] == 4 and response[2] is False for response in responses)


@pytest.mark.asyncio
async def test_duplicate_inflight_request_id_is_rejected_without_losing_active_work() -> None:
    backend = PythonRunnerBackend()
    runner = BlockingCancelRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioBinaryBridge(backend)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id=TaskLeaseId("task-lease-test"))
    run = encode_binary_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(single_test_batch(task)),
        },
    )
    cancel = encode_binary_request(
        3,
        Opcode.RUNNER_CANCEL,
        {"runner_id": "echo.runner", "invocation_id": "invocation:test"},
    )

    responses = await asyncio.wait_for(send_frames(bridge, run, run, cancel), timeout=1)
    duplicate = next(response for response in responses if response[2] is True)

    assert duplicate[0] == 2
    assert cast(dict[str, JsonValue], duplicate[3])["route"] == "wire.request_id_duplicate"
    assert any(response[0] == 2 and response[2] is False for response in responses)
    assert any(response[0] == 3 and response[2] is False for response in responses)


@pytest.mark.asyncio
async def test_protocol_stdout_is_not_polluted_by_runner_prints() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(PrintingRunner(echo_descriptor()))
    diagnostics = io.StringIO()
    bridge = StdioBinaryBridge(backend, diagnostics=diagnostics)
    await initialize(bridge)
    task = replace(Task.new("task-1", "raw.input"), lease_id=TaskLeaseId("task-lease-test"))
    run = encode_binary_request(
        2,
        Opcode.RUNNER_RUN_BATCH,
        {
            "runner_id": "echo.runner",
            "ctx": to_json_dict(runner_context()),
            "batch": to_json_dict(single_test_batch(task)),
        },
    )

    responses = await send_frames(bridge, run)

    assert "runner diagnostic" in diagnostics.getvalue()
    assert responses[0][2] is False


def test_binary_oversized_frame_is_rejected_without_unbounded_read() -> None:
    limits = WireLimits(
        max_frame_bytes=128,
        max_payload_bytes=64,
        max_inline_resource_bytes=32,
        max_in_flight_requests=2,
        management_reserved_requests=1,
    )

    encoded = struct.pack(">I", limits.max_frame_bytes + 1)

    with pytest.raises(WireProtocolFailure, match=r"wire\.frame_oversized"):
        read_binary_request(io.BytesIO(encoded), limits)


@pytest.mark.asyncio
async def test_malformed_binary_frame_closes_protocol_without_response() -> None:
    diagnostics = io.StringIO()
    output = io.BytesIO()
    bridge = StdioBinaryBridge(PythonRunnerBackend(), diagnostics=diagnostics)

    await bridge.serve(io.BytesIO(b"\x00\x00\x00\x01x"), output)

    assert output.getvalue() == b""


async def initialize(
    bridge: StdioBinaryBridge, limits: WireLimits = DEFAULT_WIRE_LIMITS
) -> None:
    hello = ProtocolHello.for_codec(BINARY_CODEC_ID, limits)
    responses = await send_frames(
        bridge,
        encode_binary_request(
            1, Opcode.PLUGIN_INITIALIZE, {"hello": hello.to_dict()}, limits
        ),
    )
    assert responses[0][0] == 1
    assert responses[0][1] is Opcode.PLUGIN_INITIALIZE
    assert responses[0][2] is False


async def send_frames(
    bridge: StdioBinaryBridge, *frames: bytes
) -> list[tuple[int, Opcode, bool, JsonValue]]:
    output = io.BytesIO()
    await bridge.serve(io.BytesIO(b"".join(frames)), output)
    return [
        cast(tuple[int, Opcode, bool, JsonValue], binary_response_payload(frame))
        for frame in _frames(output.getvalue())
    ]


def _frames(encoded: bytes) -> list[bytes]:
    frames: list[bytes] = []
    offset = 0
    while offset < len(encoded):
        body_len = struct.unpack_from(">I", encoded, offset)[0]
        end = offset + 4 + body_len
        frames.append(encoded[offset:end])
        offset = end
    return frames
