from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import cast

from mutsuki_runner_kit.contracts.codec import JsonValue, as_int, as_str, field_value
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.wire.generated import OPCODE_METHODS, Opcode
from mutsuki_runner_kit.wire.protocol import (
    DEFAULT_WIRE_LIMITS,
    WireLimits,
    WireProtocolFailure,
    WireProtocolVersion,
)
from mutsuki_runner_kit.wire.requests import RunnerWireRequest, decode_request


@dataclass(frozen=True)
class JsonlRequestFrame:
    request_id: int
    opcode: Opcode
    request: RunnerWireRequest


def decode_jsonl_request(
    encoded: bytes | str | Mapping[str, object],
    limits: WireLimits = DEFAULT_WIRE_LIMITS,
) -> JsonlRequestFrame:
    if isinstance(encoded, bytes | str):
        raw_bytes = encoded if isinstance(encoded, bytes) else encoded.encode()
        if len(raw_bytes) > limits.max_jsonl_line_bytes:
            raise WireProtocolFailure(
                "wire.frame_oversized",
                f"JSONL frame {len(raw_bytes)} > {limits.max_jsonl_line_bytes}",
            )
        loaded = json.loads(raw_bytes)
        if not isinstance(loaded, Mapping):
            raise TypeError("JSONL request expects mapping")
        raw = loaded
    else:
        raw = encoded
    request_id = as_int(field_value(raw, "request_id"), "request_id")
    if request_id <= 0:
        raise WireProtocolFailure("wire.request_id_invalid", "request_id must be positive")
    protocol_raw = field_value(raw, "protocol")
    if not isinstance(protocol_raw, Mapping):
        raise TypeError("protocol expects mapping")
    WireProtocolVersion.from_mapping(protocol_raw).ensure_compatible()
    opcode_value = as_int(field_value(raw, "opcode"), "opcode")
    try:
        opcode = Opcode(opcode_value)
    except ValueError as exc:
        raise WireProtocolFailure(
            "wire.opcode_unknown", f"unknown opcode {opcode_value:#06x}"
        ) from exc
    method = as_str(field_value(raw, "method"), "method")
    expected_method = OPCODE_METHODS[opcode]
    if method != expected_method:
        raise WireProtocolFailure(
            "wire.method_mismatch",
            f"opcode {opcode_value:#06x} maps to {expected_method}, not {method}",
        )
    payload = field_value(raw, "payload")
    if not isinstance(payload, Mapping):
        raise TypeError("payload expects mapping")
    payload_bytes = _json_bytes(payload)
    payload_len = as_int(field_value(raw, "payload_len"), "payload_len")
    if payload_len != len(payload_bytes):
        raise WireProtocolFailure(
            "wire.payload_length_mismatch",
            f"declared {payload_len}, actual {len(payload_bytes)}",
        )
    if payload_len > limits.max_payload_bytes:
        raise WireProtocolFailure(
            "wire.payload_oversized",
            f"payload {payload_len} > {limits.max_payload_bytes}",
        )
    return JsonlRequestFrame(request_id, opcode, decode_request(opcode, payload))


def encode_jsonl_request(
    request_id: int,
    opcode: Opcode,
    payload: Mapping[str, object],
    limits: WireLimits = DEFAULT_WIRE_LIMITS,
) -> bytes:
    if request_id <= 0:
        raise WireProtocolFailure(
            "wire.request_id_invalid", "request_id must be positive"
        )
    payload_bytes = _json_bytes(payload)
    if len(payload_bytes) > limits.max_payload_bytes:
        raise WireProtocolFailure(
            "wire.payload_oversized",
            f"payload {len(payload_bytes)} > {limits.max_payload_bytes}",
        )
    envelope = {
        "request_id": request_id,
        "protocol": WireProtocolVersion.current().to_dict(),
        "opcode": int(opcode),
        "method": OPCODE_METHODS[opcode],
        "payload_len": len(payload_bytes),
        "payload": dict(payload),
    }
    encoded = _json_bytes(envelope) + b"\n"
    if len(encoded) > limits.max_jsonl_line_bytes:
        raise WireProtocolFailure(
            "wire.frame_oversized",
            f"JSONL frame {len(encoded)} > {limits.max_jsonl_line_bytes}",
        )
    return encoded


def encode_jsonl_response(
    request_id: int,
    opcode: Opcode,
    result: JsonValue | None = None,
    error: RuntimeError | None = None,
    limits: WireLimits = DEFAULT_WIRE_LIMITS,
) -> bytes:
    payload: JsonValue = _runtime_error_dict(error) if error is not None else result
    payload_bytes = json.dumps(
        payload, separators=(",", ":"), ensure_ascii=False
    ).encode()
    if len(payload_bytes) > limits.max_payload_bytes:
        raise WireProtocolFailure(
            "wire.payload_oversized",
            f"payload {len(payload_bytes)} > {limits.max_payload_bytes}",
        )
    envelope: dict[str, JsonValue] = {
        "request_id": request_id,
        "protocol": cast(JsonValue, WireProtocolVersion.current().to_dict()),
        "opcode": int(opcode),
        "payload_len": len(payload_bytes),
        "ok": error is None,
        "result": None if error is not None else result,
        "error": _runtime_error_dict(error) if error is not None else None,
    }
    encoded = json.dumps(envelope, separators=(",", ":"), ensure_ascii=False).encode() + b"\n"
    if len(encoded) > limits.max_jsonl_line_bytes:
        raise WireProtocolFailure(
            "wire.frame_oversized",
            f"JSONL frame {len(encoded)} > {limits.max_jsonl_line_bytes}",
        )
    return encoded


def response_dict(encoded: bytes) -> dict[str, JsonValue]:
    loaded = json.loads(encoded)
    if not isinstance(loaded, dict):
        raise TypeError("response expects mapping")
    return cast(dict[str, JsonValue], loaded)


def safe_request_identity(request: object) -> tuple[int, Opcode] | None:
    if not isinstance(request, Mapping):
        return None
    request_id = request.get("request_id")
    opcode = request.get("opcode")
    if not isinstance(request_id, int) or isinstance(request_id, bool) or request_id <= 0:
        return None
    if not isinstance(opcode, int) or isinstance(opcode, bool):
        return None
    try:
        return request_id, Opcode(opcode)
    except ValueError:
        return None


def _runtime_error_dict(error: RuntimeError | None) -> JsonValue:
    if error is None:
        return None
    from mutsuki_runner_kit.contracts.codec import to_json_dict

    return to_json_dict(error)


def _json_bytes(value: object) -> bytes:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()
