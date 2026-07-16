from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Self

from mutsuki_runner_kit.contracts.codec import JsonDict, as_str, field_value, to_json_dict
from mutsuki_runner_kit.contracts.plugin import PluginManifest
from mutsuki_runner_kit.wire.protocol import (
    DEFAULT_WIRE_LIMITS,
    SCHEMA_REVISION,
    ProtocolHello,
    WireLimits,
    WireProtocolFailure,
    WireProtocolVersion,
)


@dataclass(frozen=True)
class ProtocolHelloAck(ProtocolHello):
    plugin: InitializedPlugin | None = None

    @classmethod
    def negotiate(
        cls,
        hello: ProtocolHello,
        expected_codec: str,
        plugin: InitializedPlugin | None = None,
        limits: WireLimits = DEFAULT_WIRE_LIMITS,
    ) -> Self:
        limits.validate()
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
        _validate_offered_limits(hello)
        required = ProtocolHello.for_codec(expected_codec)
        _validate_required_features(required, hello)
        return cls(
            protocol=WireProtocolVersion.current(),
            codec_id=expected_codec,
            schema_revision=SCHEMA_REVISION,
            max_frame_bytes=min(hello.max_frame_bytes, limits.max_frame_bytes),
            max_payload_bytes=min(hello.max_payload_bytes, limits.max_payload_bytes),
            max_in_flight_requests=min(hello.max_in_flight_requests, limits.max_in_flight_requests),
            management_reserved_requests=min(
                hello.management_reserved_requests,
                limits.management_reserved_requests,
                hello.max_in_flight_requests - 1,
            ),
            management_channel=hello.management_channel,
            feature_flags=hello.feature_flags,
            plugin=plugin,
        )

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> Self:
        hello = ProtocolHello.from_mapping(raw)
        plugin = raw.get("plugin")
        return cls(
            **hello.__dict__,
            plugin=None
            if plugin is None
            else InitializedPlugin.from_mapping(_as_mapping(plugin, "plugin")),
        )

    def validate_for(self, hello: ProtocolHello) -> None:
        self.protocol.ensure_compatible()
        if self.codec_id != hello.codec_id:
            raise WireProtocolFailure(
                "wire.codec_mismatch", f"expected {hello.codec_id}, got {self.codec_id}"
            )
        if self.schema_revision != hello.schema_revision:
            raise WireProtocolFailure(
                "wire.schema_mismatch",
                f"expected {hello.schema_revision}, got {self.schema_revision}",
            )
        if (
            self.max_frame_bytes <= 0
            or self.max_frame_bytes > hello.max_frame_bytes
            or self.max_payload_bytes <= 0
            or self.max_payload_bytes > hello.max_payload_bytes
            or self.max_in_flight_requests <= 0
            or self.max_in_flight_requests > hello.max_in_flight_requests
            or self.management_reserved_requests <= 0
            or self.management_reserved_requests > hello.management_reserved_requests
            or self.management_reserved_requests >= self.max_in_flight_requests
        ):
            raise WireProtocolFailure(
                "wire.limit_mismatch",
                "wire peer returned invalid or expanded negotiated limits",
            )
        _validate_required_features(hello, self)

    def to_dict(self) -> JsonDict:
        value = super().to_dict()
        if self.plugin is not None:
            value["plugin"] = self.plugin.to_dict()
        return value


@dataclass(frozen=True)
class InitializedPlugin:
    manifest: PluginManifest
    resource_provider_ids: tuple[str, ...]

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> Self:
        provider_ids = field_value(raw, "resource_provider_ids")
        if not isinstance(provider_ids, Sequence) or isinstance(
            provider_ids, str | bytes | bytearray
        ):
            raise TypeError("resource_provider_ids expects sequence")
        return cls(
            manifest=PluginManifest.from_json_dict(
                _as_mapping(field_value(raw, "manifest"), "manifest")
            ),
            resource_provider_ids=tuple(
                as_str(provider_id, "resource_provider_ids") for provider_id in provider_ids
            ),
        )

    def to_dict(self) -> JsonDict:
        return {
            "manifest": to_json_dict(self.manifest),
            "resource_provider_ids": list(self.resource_provider_ids),
        }


def _validate_offered_limits(hello: ProtocolHello) -> None:
    if (
        hello.max_frame_bytes <= 0
        or hello.max_payload_bytes <= 0
        or hello.max_in_flight_requests <= 0
        or hello.management_reserved_requests <= 0
        or hello.management_reserved_requests >= hello.max_in_flight_requests
    ):
        raise WireProtocolFailure("wire.limit_mismatch", "wire limits must be greater than zero")


def _validate_required_features(required: ProtocolHello, offered: ProtocolHello) -> None:
    if required.management_channel and not offered.management_channel:
        raise WireProtocolFailure(
            "wire.management_channel_required",
            "wire management channel support is required",
        )
    missing = next(
        (flag for flag in required.feature_flags if flag not in offered.feature_flags),
        None,
    )
    if missing is not None:
        raise WireProtocolFailure(
            "wire.feature_missing", f"wire peer is missing required feature {missing}"
        )


def _as_mapping(value: object, name: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise TypeError(f"{name} expects mapping")
    return value
