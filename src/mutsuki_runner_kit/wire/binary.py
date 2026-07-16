from __future__ import annotations

import struct
from collections.abc import Mapping
from dataclasses import dataclass
from typing import BinaryIO, cast

import msgpack

from mutsuki_runner_kit.contracts.codec import JsonValue, to_json_dict
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.wire.generated import MANAGEMENT_OPCODES, Opcode
from mutsuki_runner_kit.wire.msgpack_structure import (
    MAX_MSGPACK_CONTAINER_ITEMS,
    MAX_MSGPACK_NESTING_DEPTH,
    validate_messagepack_structure,
)
from mutsuki_runner_kit.wire.protocol import (
    DEFAULT_WIRE_LIMITS,
    WireLimits,
    WireProtocolFailure,
    WireProtocolVersion,
)
from mutsuki_runner_kit.wire.requests import RunnerWireRequest, decode_request

MAGIC = 0x4D555453
LENGTH_PREFIX_SIZE = 4
HEADER_SIZE = 24
FLAG_REQUEST = 0x0001
FLAG_RESPONSE = 0x0002
FLAG_ERROR = 0x0004
FLAG_MANAGEMENT = 0x0008
KNOWN_FLAGS = FLAG_REQUEST | FLAG_RESPONSE | FLAG_ERROR | FLAG_MANAGEMENT
_HEADER = struct.Struct(">IHHHHQI")
_LENGTH = struct.Struct(">I")


@dataclass(frozen=True)
class BinaryRequestFrame:
    request_id: int
    opcode: Opcode
    request: RunnerWireRequest


def encode_binary_request(
    request_id: int,
    opcode: Opcode,
    payload: Mapping[str, object],
    limits: WireLimits = DEFAULT_WIRE_LIMITS,
) -> bytes:
    _validate_value(payload, limits)
    packed = cast(bytes, msgpack.packb(dict(payload), use_bin_type=True))
    flags = FLAG_REQUEST | (FLAG_MANAGEMENT if opcode in MANAGEMENT_OPCODES else 0)
    return _encode_frame(request_id, opcode, flags, packed, limits)


def decode_binary_request(
    encoded: bytes, limits: WireLimits = DEFAULT_WIRE_LIMITS
) -> BinaryRequestFrame:
    request_id, opcode, flags, payload = _decode_frame(encoded, limits)
    if flags & FLAG_REQUEST == 0 or flags & FLAG_RESPONSE:
        raise WireProtocolFailure("wire.flags_invalid", f"invalid request flags {flags:#06x}")
    unpacked = _unpack(payload, limits)
    if not isinstance(unpacked, Mapping):
        raise TypeError("MessagePack request payload expects mapping")
    _validate_value(unpacked, limits)
    return BinaryRequestFrame(request_id, opcode, decode_request(opcode, unpacked))


def encode_binary_response(
    request_id: int,
    opcode: Opcode,
    result: JsonValue | None = None,
    error: RuntimeError | None = None,
    limits: WireLimits = DEFAULT_WIRE_LIMITS,
) -> bytes:
    payload: object = to_json_dict(error) if error is not None else result
    _validate_value(payload, limits)
    packed = cast(bytes, msgpack.packb(payload, use_bin_type=True))
    flags = FLAG_RESPONSE | (FLAG_ERROR if error is not None else 0)
    return _encode_frame(request_id, opcode, flags, packed, limits)


def read_binary_request(
    stream: BinaryIO, limits: WireLimits = DEFAULT_WIRE_LIMITS
) -> BinaryRequestFrame | None:
    prefix = stream.read(LENGTH_PREFIX_SIZE)
    if prefix == b"":
        return None
    if len(prefix) != LENGTH_PREFIX_SIZE:
        raise WireProtocolFailure(
            "wire.frame_truncated",
            f"length prefix expected {LENGTH_PREFIX_SIZE}, got {len(prefix)}",
        )
    (body_len,) = _LENGTH.unpack(prefix)
    _validate_body_len(body_len, limits)
    body = _read_exact(stream, body_len)
    return decode_binary_request(prefix + body, limits)


def binary_response_payload(
    encoded: bytes, limits: WireLimits = DEFAULT_WIRE_LIMITS
) -> tuple[int, Opcode, bool, object]:
    request_id, opcode, flags, payload = _decode_frame(encoded, limits)
    if flags & FLAG_RESPONSE == 0 or flags & FLAG_REQUEST:
        raise WireProtocolFailure("wire.flags_invalid", f"invalid response flags {flags:#06x}")
    return request_id, opcode, bool(flags & FLAG_ERROR), _unpack(payload, limits)


def _encode_frame(
    request_id: int,
    opcode: Opcode,
    flags: int,
    payload: bytes,
    limits: WireLimits,
) -> bytes:
    if request_id <= 0:
        raise WireProtocolFailure("wire.request_id_invalid", "request_id must be positive")
    if len(payload) > limits.max_payload_bytes:
        raise WireProtocolFailure(
            "wire.payload_oversized",
            f"payload {len(payload)} > {limits.max_payload_bytes}",
        )
    body_len = HEADER_SIZE + len(payload)
    _validate_body_len(body_len, limits)
    version = WireProtocolVersion.current()
    header = _HEADER.pack(
        MAGIC,
        version.major,
        version.minor,
        int(opcode),
        flags,
        request_id,
        len(payload),
    )
    return _LENGTH.pack(body_len) + header + payload


def _decode_frame(
    encoded: bytes, limits: WireLimits
) -> tuple[int, Opcode, int, bytes]:
    if len(encoded) < LENGTH_PREFIX_SIZE:
        raise WireProtocolFailure("wire.frame_truncated", "missing length prefix")
    (body_len,) = _LENGTH.unpack_from(encoded)
    _validate_body_len(body_len, limits)
    actual_body_len = len(encoded) - LENGTH_PREFIX_SIZE
    if actual_body_len != body_len:
        raise WireProtocolFailure(
            "wire.frame_truncated",
            f"declared {body_len}, actual {actual_body_len}",
        )
    magic, major, minor, opcode_raw, flags, request_id, payload_len = _HEADER.unpack_from(
        encoded, LENGTH_PREFIX_SIZE
    )
    if magic != MAGIC:
        raise WireProtocolFailure("wire.magic_invalid", f"invalid magic {magic:#010x}")
    WireProtocolVersion(major, minor).ensure_compatible()
    if flags & ~KNOWN_FLAGS:
        raise WireProtocolFailure("wire.flags_invalid", f"unknown flags {flags:#06x}")
    if bool(flags & FLAG_REQUEST) == bool(flags & FLAG_RESPONSE):
        raise WireProtocolFailure(
            "wire.flags_invalid", "exactly one of request/response is required"
        )
    if flags & FLAG_ERROR and not flags & FLAG_RESPONSE:
        raise WireProtocolFailure("wire.flags_invalid", "error flag requires response")
    if request_id <= 0:
        raise WireProtocolFailure("wire.request_id_invalid", "request_id must be positive")
    try:
        opcode = Opcode(opcode_raw)
    except ValueError as exc:
        raise WireProtocolFailure(
            "wire.opcode_unknown", f"unknown opcode {opcode_raw:#06x}"
        ) from exc
    payload = encoded[LENGTH_PREFIX_SIZE + HEADER_SIZE :]
    if payload_len != len(payload):
        raise WireProtocolFailure(
            "wire.payload_length_mismatch",
            f"declared {payload_len}, actual {len(payload)}",
        )
    if payload_len > limits.max_payload_bytes:
        raise WireProtocolFailure(
            "wire.payload_oversized",
            f"payload {payload_len} > {limits.max_payload_bytes}",
        )
    return request_id, opcode, flags, payload


def _unpack(payload: bytes, limits: WireLimits) -> object:
    validate_messagepack_structure(payload)
    try:
        return msgpack.unpackb(
            payload,
            raw=False,
            strict_map_key=True,
            max_str_len=limits.max_payload_bytes,
            max_bin_len=limits.max_inline_resource_bytes,
            max_array_len=MAX_MSGPACK_CONTAINER_ITEMS,
            max_map_len=MAX_MSGPACK_CONTAINER_ITEMS,
            max_ext_len=0,
        )
    except (ValueError, msgpack.UnpackException) as exc:
        raise WireProtocolFailure("wire.msgpack_malformed", str(exc)) from exc


def _validate_body_len(body_len: int, limits: WireLimits) -> None:
    if body_len < HEADER_SIZE:
        raise WireProtocolFailure(
            "wire.frame_truncated", f"body {body_len} < header {HEADER_SIZE}"
        )
    if body_len > limits.max_frame_bytes:
        raise WireProtocolFailure(
            "wire.frame_oversized",
            f"frame {body_len} > {limits.max_frame_bytes}",
        )


def _read_exact(stream: BinaryIO, length: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < length:
        chunk = stream.read(length - len(chunks))
        if not chunk:
            raise WireProtocolFailure(
                "wire.frame_truncated",
                f"expected {length}, got {len(chunks)}",
            )
        chunks.extend(chunk)
    return bytes(chunks)


def _validate_value(value: object, limits: WireLimits, depth: int = 0) -> None:
    if depth > MAX_MSGPACK_NESTING_DEPTH:
        raise WireProtocolFailure(
            "wire.msgpack_depth", "MessagePack nesting depth exceeded"
        )
    if isinstance(value, bytes | bytearray | memoryview):
        if len(value) > limits.max_inline_resource_bytes:
            raise WireProtocolFailure(
                "wire.inline_resource_oversized",
                f"inline bytes {len(value)} > {limits.max_inline_resource_bytes}; use ResourceRef",
            )
        return
    if isinstance(value, Mapping):
        if len(value) > MAX_MSGPACK_CONTAINER_ITEMS:
            raise WireProtocolFailure(
                "wire.msgpack_container_limit",
                f"mapping {len(value)} > {MAX_MSGPACK_CONTAINER_ITEMS}",
            )
        for item in value.values():
            _validate_value(item, limits, depth + 1)
        return
    if isinstance(value, list | tuple):
        if len(value) > MAX_MSGPACK_CONTAINER_ITEMS:
            raise WireProtocolFailure(
                "wire.msgpack_container_limit",
                f"sequence {len(value)} > {MAX_MSGPACK_CONTAINER_ITEMS}",
            )
        for item in value:
            _validate_value(item, limits, depth + 1)
