from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Self

from mutsuki_runner_kit.contracts.codec import JsonDict, as_bool, as_int, as_str, field_value
from mutsuki_runner_kit.wire.schema import RUNTIME_WIRE_SCHEMA

DEBUG_JSONL_CODEC_ID = "mutsuki.codec.typed-jsonl.v1"
BINARY_CODEC_ID = "mutsuki.codec.typed-msgpack.v1"
SCHEMA_REVISION = "mutsuki.runtime.wire/1.0.0"


@dataclass(frozen=True)
class WireLimits:
    max_frame_bytes: int
    max_payload_bytes: int
    max_jsonl_line_bytes: int
    max_inline_resource_bytes: int
    max_in_flight_requests: int
    management_reserved_requests: int


_limits = RUNTIME_WIRE_SCHEMA["limits"]
_resource_policy = RUNTIME_WIRE_SCHEMA["resource_policy"]
if not isinstance(_limits, dict) or not isinstance(_resource_policy, dict):
    raise TypeError("runtime wire schema limits are invalid")

DEFAULT_WIRE_LIMITS = WireLimits(
    max_frame_bytes=as_int(field_value(_limits, "max_frame_bytes"), "max_frame_bytes"),
    max_payload_bytes=as_int(field_value(_limits, "max_payload_bytes"), "max_payload_bytes"),
    max_jsonl_line_bytes=as_int(
        field_value(_limits, "max_jsonl_line_bytes"), "max_jsonl_line_bytes"
    ),
    max_inline_resource_bytes=as_int(
        field_value(_resource_policy, "inline_limit_bytes"), "inline_limit_bytes"
    ),
    max_in_flight_requests=as_int(
        field_value(_limits, "max_in_flight_requests"), "max_in_flight_requests"
    ),
    management_reserved_requests=as_int(
        field_value(_limits, "management_reserved_requests"),
        "management_reserved_requests",
    ),
)


@dataclass(frozen=True)
class WireProtocolVersion:
    major: int
    minor: int

    @classmethod
    def current(cls) -> Self:
        return cls(major=1, minor=0)

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> Self:
        return cls(
            major=as_int(field_value(raw, "major"), "major"),
            minor=as_int(field_value(raw, "minor"), "minor"),
        )

    def ensure_compatible(self) -> None:
        if self.major != self.current().major:
            raise WireProtocolFailure(
                "wire.version_mismatch",
                f"expected major {self.current().major}, got {self.major}",
            )

    def to_dict(self) -> JsonDict:
        return {"major": self.major, "minor": self.minor}


@dataclass(frozen=True)
class ProtocolHello:
    protocol: WireProtocolVersion
    codec_id: str
    schema_revision: str
    max_frame_bytes: int
    max_payload_bytes: int
    max_in_flight_requests: int
    management_channel: bool
    feature_flags: tuple[str, ...]

    @classmethod
    def for_codec(cls, codec_id: str) -> Self:
        return cls(
            protocol=WireProtocolVersion.current(),
            codec_id=codec_id,
            schema_revision=SCHEMA_REVISION,
            max_frame_bytes=DEFAULT_WIRE_LIMITS.max_frame_bytes,
            max_payload_bytes=DEFAULT_WIRE_LIMITS.max_payload_bytes,
            max_in_flight_requests=DEFAULT_WIRE_LIMITS.max_in_flight_requests,
            management_channel=True,
            feature_flags=(
                "typed_requests",
                "out_of_order_responses",
                "resource_ref_required_for_large_bytes",
            ),
        )

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> Self:
        protocol = field_value(raw, "protocol")
        flags = field_value(raw, "feature_flags")
        if not isinstance(protocol, Mapping):
            raise TypeError("protocol expects mapping")
        if not isinstance(flags, Sequence) or isinstance(flags, str | bytes | bytearray):
            raise TypeError("feature_flags expects sequence")
        return cls(
            protocol=WireProtocolVersion.from_mapping(protocol),
            codec_id=as_str(field_value(raw, "codec_id"), "codec_id"),
            schema_revision=as_str(
                field_value(raw, "schema_revision"), "schema_revision"
            ),
            max_frame_bytes=as_int(
                field_value(raw, "max_frame_bytes"), "max_frame_bytes"
            ),
            max_payload_bytes=as_int(
                field_value(raw, "max_payload_bytes"), "max_payload_bytes"
            ),
            max_in_flight_requests=as_int(
                field_value(raw, "max_in_flight_requests"), "max_in_flight_requests"
            ),
            management_channel=as_bool(
                field_value(raw, "management_channel"), "management_channel"
            ),
            feature_flags=tuple(as_str(flag, "feature_flags") for flag in flags),
        )

    def to_dict(self) -> JsonDict:
        return {
            "protocol": self.protocol.to_dict(),
            "codec_id": self.codec_id,
            "schema_revision": self.schema_revision,
            "max_frame_bytes": self.max_frame_bytes,
            "max_payload_bytes": self.max_payload_bytes,
            "max_in_flight_requests": self.max_in_flight_requests,
            "management_channel": self.management_channel,
            "feature_flags": list(self.feature_flags),
        }


@dataclass(frozen=True)
class ProtocolHelloAck(ProtocolHello):
    @classmethod
    def negotiate(cls, hello: ProtocolHello, expected_codec: str) -> Self:
        hello.protocol.ensure_compatible()
        if hello.codec_id != expected_codec:
            raise WireProtocolFailure(
                "wire.codec_mismatch",
                f"expected {expected_codec}, got {hello.codec_id}",
            )
        if hello.schema_revision != SCHEMA_REVISION:
            raise WireProtocolFailure(
                "wire.schema_mismatch",
                f"expected {SCHEMA_REVISION}, got {hello.schema_revision}",
            )
        limits = DEFAULT_WIRE_LIMITS
        return cls(
            protocol=WireProtocolVersion.current(),
            codec_id=expected_codec,
            schema_revision=SCHEMA_REVISION,
            max_frame_bytes=min(hello.max_frame_bytes, limits.max_frame_bytes),
            max_payload_bytes=min(hello.max_payload_bytes, limits.max_payload_bytes),
            max_in_flight_requests=min(
                hello.max_in_flight_requests, limits.max_in_flight_requests
            ),
            management_channel=True,
            feature_flags=tuple(
                flag
                for flag in ProtocolHello.for_codec(expected_codec).feature_flags
                if flag in hello.feature_flags
            ),
        )


class WireProtocolFailure(ValueError):
    def __init__(self, code: str, detail: str) -> None:
        self.code = code
        self.detail = detail
        super().__init__(f"{code}: {detail}")
