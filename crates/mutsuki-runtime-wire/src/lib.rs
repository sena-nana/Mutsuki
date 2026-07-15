//! Versioned, language-neutral Runtime Wire registry and codecs.
//!
//! Runtime DTOs remain owned by `mutsuki-runtime-contracts`. This crate is the
//! single source for closed operation identifiers, request/response pairing,
//! compatibility negotiation, and transport-independent framing.

mod binary;
mod jsonl;
mod operations;
mod protocol;
mod schema;

pub use binary::{
    BINARY_HEADER_LEN, BINARY_LENGTH_PREFIX_LEN, BinaryFrame, WireFlags, WireHeader,
    decode_binary_frame, decode_binary_payload, encode_binary_request, encode_binary_response,
    read_binary_frame,
};
pub use jsonl::{
    JsonlRequestEnvelope, JsonlResponseEnvelope, decode_jsonl_request, decode_jsonl_response,
    encode_jsonl_request, encode_jsonl_response,
};
pub use operations::*;
pub use protocol::{
    BINARY_CODEC_ID, DEBUG_JSONL_CODEC_ID, DEFAULT_WIRE_LIMITS, MANAGEMENT_RESERVED_REQUESTS,
    MAX_FRAME_BYTES, MAX_IN_FLIGHT_REQUESTS, MAX_INLINE_RESOURCE_BYTES, MAX_JSONL_LINE_BYTES,
    MAX_PAYLOAD_BYTES, Opcode, ProtocolHello, ProtocolHelloAck, SCHEMA_REVISION, WireCodecError,
    WireLimits, WireProtocolVersion, WireRequest,
};
pub use schema::{
    generated_fixtures_json, generated_fixtures_value, generated_schema_json,
    generated_schema_value,
};

#[cfg(test)]
mod tests;
