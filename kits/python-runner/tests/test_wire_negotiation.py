from __future__ import annotations

import io
import json
from dataclasses import replace
from typing import cast

import pytest

from mutsuki_runner_kit.contracts.codec import JsonValue
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.transport.stdio_binary import StdioBinaryBridge
from mutsuki_runner_kit.transport.stdio_jsonl import StdioJsonlBridge
from mutsuki_runner_kit.wire.binary import encode_binary_request
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.jsonl import encode_jsonl_request
from mutsuki_runner_kit.wire.protocol import (
    BINARY_CODEC_ID,
    DEBUG_JSONL_CODEC_ID,
    DEFAULT_WIRE_LIMITS,
    ProtocolHello,
    WireLimits,
    WireProtocolFailure,
)


def offered_limits() -> WireLimits:
    return replace(
        DEFAULT_WIRE_LIMITS,
        max_in_flight_requests=3,
        management_reserved_requests=1,
    )


@pytest.mark.asyncio
async def test_jsonl_connection_enforces_negotiated_management_reservation() -> None:
    limits = offered_limits()
    hello = ProtocolHello.for_codec(DEBUG_JSONL_CODEC_ID, limits)
    request = json.loads(
        encode_jsonl_request(1, Opcode.PLUGIN_INITIALIZE, {"hello": hello.to_dict()}, limits)
    )
    bridge = StdioJsonlBridge(PythonRunnerBackend())

    response = await bridge.handle_request(request)

    result = cast(dict[str, JsonValue], response["result"])
    assert result["max_in_flight_requests"] == 3
    assert result["management_reserved_requests"] == 1
    assert bridge.limits.max_in_flight_requests == 3
    assert bridge.limits.management_reserved_requests == 1

    repeated = await bridge.handle_request(request)
    assert cast(dict[str, JsonValue], repeated["error"])["route"] == ("wire.already_initialized")


@pytest.mark.asyncio
async def test_binary_connection_uses_the_same_negotiated_limits() -> None:
    limits = offered_limits()
    hello = ProtocolHello.for_codec(BINARY_CODEC_ID, limits)
    request = encode_binary_request(1, Opcode.PLUGIN_INITIALIZE, {"hello": hello.to_dict()}, limits)
    bridge = StdioBinaryBridge(PythonRunnerBackend())

    await bridge.serve(io.BytesIO(request), io.BytesIO())

    assert bridge.limits.max_in_flight_requests == 3
    assert bridge.limits.management_reserved_requests == 1


def test_invalid_wire_limit_reservation_is_rejected_before_startup() -> None:
    invalid = replace(
        DEFAULT_WIRE_LIMITS,
        max_in_flight_requests=2,
        management_reserved_requests=2,
    )

    with pytest.raises(WireProtocolFailure, match=r"wire\.limit_mismatch"):
        StdioJsonlBridge(PythonRunnerBackend(), limits=invalid)
