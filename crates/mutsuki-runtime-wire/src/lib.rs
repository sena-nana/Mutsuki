//! Versioned, language-neutral Runtime Wire registry and codecs.
//!
//! Runtime DTOs remain owned by `mutsuki-runtime-contracts`. This crate is the
//! single source for closed operation identifiers, request/response pairing,
//! compatibility negotiation, and transport-independent framing.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::format_collect,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

mod binary;
mod operations;
mod protocol;
mod schema;

pub use binary::{
    BINARY_HEADER_LEN, BINARY_LENGTH_PREFIX_LEN, BinaryFrame, MAX_MSGPACK_CONTAINER_ITEMS,
    MAX_MSGPACK_NESTING_DEPTH, WireFlags, WireHeader, decode_binary_any_request,
    decode_binary_frame, decode_binary_payload, decode_binary_request, decode_binary_response,
    encode_binary_request, encode_binary_response, read_binary_frame, read_binary_frame_bytes,
};
pub use operations::*;
pub use protocol::{
    BINARY_CODEC_ID, DEFAULT_WIRE_LIMITS, InitializedPlugin, MANAGEMENT_RESERVED_REQUESTS,
    MAX_FRAME_BYTES, MAX_IN_FLIGHT_REQUESTS, MAX_INLINE_RESOURCE_BYTES, MAX_PAYLOAD_BYTES, Opcode,
    ProtocolHello, ProtocolHelloAck, SCHEMA_REVISION, WireCodecError, WireLimits,
    WireProtocolVersion, WireRequest,
};
pub use schema::{
    generated_binary_golden_json, generated_binary_golden_value, generated_fixtures_json,
    generated_fixtures_value, generated_schema_json, generated_schema_value,
};

#[cfg(test)]
mod tests;
