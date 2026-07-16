"""Generated and schema-validated Runtime Wire protocol surface."""

from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.handshake import InitializedPlugin, ProtocolHelloAck
from mutsuki_runner_kit.wire.protocol import (
    BINARY_CODEC_ID,
    DEBUG_JSONL_CODEC_ID,
    SCHEMA_REVISION,
    ProtocolHello,
    WireLimits,
    WireProtocolVersion,
)

__all__ = [
    "BINARY_CODEC_ID",
    "DEBUG_JSONL_CODEC_ID",
    "SCHEMA_REVISION",
    "InitializedPlugin",
    "Opcode",
    "ProtocolHello",
    "ProtocolHelloAck",
    "WireLimits",
    "WireProtocolVersion",
]
