from __future__ import annotations

import io
import struct
from dataclasses import replace
from typing import cast

import pytest

from mutsuki_runner_kit.contracts.codec import to_json_dict
from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.batches import runner_context, single_test_batch
from mutsuki_runner_kit.testing.runners import EchoRunner, echo_descriptor
from mutsuki_runner_kit.transport.stdio_binary import StdioBinaryBridge
from mutsuki_runner_kit.wire.binary import (
    binary_response_payload,
    decode_binary_request,
    encode_binary_request,
)
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.protocol import (
    BINARY_CODEC_ID,
    DEFAULT_WIRE_LIMITS,
    ProtocolHello,
    WireProtocolFailure,
)
from mutsuki_runner_kit.wire.requests import InitializeRequest


def test_binary_codec_uses_fixed_header_and_typed_messagepack() -> None:
    hello = ProtocolHello.for_codec(BINARY_CODEC_ID)
    encoded = encode_binary_request(
        1, Opcode.PLUGIN_INITIALIZE, {"hello": hello.to_dict()}
    )
    decoded = decode_binary_request(encoded)

    assert decoded.request_id == 1
    assert decoded.opcode is Opcode.PLUGIN_INITIALIZE
    assert decoded.request == InitializeRequest(hello)
    assert len(encoded) > 28


def test_binary_codec_rejects_oversized_prefix_before_reading_payload() -> None:
    encoded = struct.pack(">I", DEFAULT_WIRE_LIMITS.max_frame_bytes + 1)

    with pytest.raises(WireProtocolFailure, match=r"wire\.frame_oversized"):
        decode_binary_request(encoded)


def test_binary_codec_rejects_large_inline_bytes_in_favor_of_resource_ref() -> None:
    payload = {
        "provider_id": "provider-a",
        "schema": "bytes.v1",
        "bytes": b"x" * (DEFAULT_WIRE_LIMITS.max_inline_resource_bytes + 1),
    }

    with pytest.raises(WireProtocolFailure, match="use ResourceRef"):
        encode_binary_request(2, Opcode.RESOURCE_CREATE_BLOB, payload)


def test_binary_codec_rejects_malformed_messagepack_with_bounded_error() -> None:
    encoded = bytearray(
        encode_binary_request(
            2,
            Opcode.RUNNER_DISPOSE,
            {"runner_id": "echo.runner"},
        )
    )
    encoded[-1] = 0xC1

    with pytest.raises(WireProtocolFailure, match=r"wire\.msgpack_malformed"):
        decode_binary_request(bytes(encoded))


@pytest.mark.parametrize(
    ("payload", "route"),
    [
        (b"\xdd\xff\xff\xff\xff", "wire.msgpack_container_limit"),
        (b"\x91" * 65 + b"\xc0", "wire.msgpack_depth"),
        (b"\x80\xc0", "wire.msgpack_trailing"),
    ],
)
def test_binary_codec_preflights_messagepack_structure(
    payload: bytes, route: str
) -> None:
    encoded = encode_binary_request(
        2, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"}
    )

    with pytest.raises(WireProtocolFailure, match=route.replace(".", r"\.")):
        decode_binary_request(_replace_payload(encoded, payload))


def test_binary_codec_rejects_conflicting_request_response_flags() -> None:
    encoded = bytearray(
        encode_binary_request(
            2, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"}
        )
    )
    struct.pack_into(">H", encoded, 14, 0x0003)

    with pytest.raises(WireProtocolFailure, match=r"wire\.flags_invalid"):
        decode_binary_request(bytes(encoded))


@pytest.mark.asyncio
async def test_binary_bridge_negotiates_before_dispatch_and_reuses_semantics() -> None:
    backend = PythonRunnerBackend()
    runner = EchoRunner(echo_descriptor())
    backend.register_runner(runner)
    bridge = StdioBinaryBridge(backend)
    hello = encode_binary_request(
        1,
        Opcode.PLUGIN_INITIALIZE,
        {"hello": ProtocolHello.for_codec(BINARY_CODEC_ID).to_dict()},
    )
    hello_output = io.BytesIO()
    await bridge.serve(io.BytesIO(hello), hello_output)
    hello_response = _frames(hello_output.getvalue())[0]
    request_id, opcode, is_error, payload = binary_response_payload(hello_response)

    assert (request_id, opcode, is_error) == (1, Opcode.PLUGIN_INITIALIZE, False)
    hello_payload = cast(dict[str, object], payload)
    assert hello_payload["codec_id"] == BINARY_CODEC_ID

    cancel = encode_binary_request(
        2,
        Opcode.RUNNER_CANCEL,
        {"runner_id": "echo.runner", "invocation_id": "inv-1"},
    )
    dispose = encode_binary_request(
        3, Opcode.RUNNER_DISPOSE, {"runner_id": "echo.runner"}
    )
    output = io.BytesIO()
    await bridge.serve(io.BytesIO(cancel + dispose), output)
    responses = [binary_response_payload(frame) for frame in _frames(output.getvalue())]

    assert {response[0] for response in responses} == {2, 3}
    assert all(not response[2] for response in responses)
    assert runner.cancelled == ["inv-1"]
    assert runner.disposed is True


@pytest.mark.asyncio
async def test_binary_and_jsonl_share_batch_contract_semantics() -> None:
    backend = PythonRunnerBackend()
    backend.register_runner(EchoRunner(echo_descriptor()))
    bridge = StdioBinaryBridge(backend)
    await bridge.serve(
        io.BytesIO(
            encode_binary_request(
                1,
                Opcode.PLUGIN_INITIALIZE,
                {"hello": ProtocolHello.for_codec(BINARY_CODEC_ID).to_dict()},
            )
        ),
        io.BytesIO(),
    )
    task = replace(Task.new("task-1", "raw.input"), lease_id="task-lease-test")
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
    output = io.BytesIO()

    await bridge.serve(io.BytesIO(run), output)
    _, _, is_error, completion = binary_response_payload(_frames(output.getvalue())[0])

    assert is_error is False
    completion_payload = cast(dict[str, object], completion)
    assert completion_payload["batch_id"] == "batch-test"
    results = cast(list[dict[str, object]], completion_payload["results"])
    assert results[0]["task_id"] == "task-1"


def _frames(encoded: bytes) -> list[bytes]:
    frames: list[bytes] = []
    offset = 0
    while offset < len(encoded):
        body_len = struct.unpack_from(">I", encoded, offset)[0]
        end = offset + 4 + body_len
        frames.append(encoded[offset:end])
        offset = end
    return frames


def _replace_payload(encoded: bytes, payload: bytes) -> bytes:
    frame = bytearray(encoded[:28])
    struct.pack_into(">I", frame, 0, 24 + len(payload))
    struct.pack_into(">I", frame, 24, len(payload))
    frame.extend(payload)
    return bytes(frame)
