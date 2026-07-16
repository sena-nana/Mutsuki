mod frame;
mod payload;

use mutsuki_runtime_contracts::RuntimeError;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::operations::decode_any_wire_request;
use crate::{DecodedWireRequest, Opcode, WireCodecError, WireLimits, WireRequest};

pub use frame::{
    BINARY_HEADER_LEN, BINARY_LENGTH_PREFIX_LEN, BinaryFrame, WireFlags, WireHeader,
    decode_binary_frame, read_binary_frame, read_binary_frame_bytes,
};
pub use payload::{MAX_MSGPACK_CONTAINER_ITEMS, MAX_MSGPACK_NESTING_DEPTH, decode_binary_payload};

use frame::encode_frame;
use payload::encode_messagepack;

pub fn encode_binary_request<R: WireRequest>(
    request_id: u64,
    request: &R,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    request.validate(limits)?;
    let payload = encode_messagepack(request)?;
    let flags = if R::OPCODE.is_management() {
        WireFlags::REQUEST | WireFlags::MANAGEMENT
    } else {
        WireFlags::REQUEST
    };
    encode_frame(R::OPCODE, flags, request_id, payload, limits)
}

pub(crate) fn encode_binary_request_value<T: Serialize>(
    request_id: u64,
    opcode: Opcode,
    request: &T,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    let payload = encode_messagepack(request)?;
    let flags = if opcode.is_management() {
        WireFlags::REQUEST | WireFlags::MANAGEMENT
    } else {
        WireFlags::REQUEST
    };
    encode_frame(opcode, flags, request_id, payload, limits)
}

pub fn encode_binary_response<T: Serialize>(
    request_id: u64,
    opcode: Opcode,
    response: Result<&T, &RuntimeError>,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    let (flags, payload) = match response {
        Ok(value) => (WireFlags::RESPONSE, encode_messagepack(value)?),
        Err(error) => (
            WireFlags::RESPONSE | WireFlags::ERROR,
            encode_messagepack(error)?,
        ),
    };
    encode_frame(opcode, flags, request_id, payload, limits)
}

pub fn decode_binary_request<R>(
    bytes: &[u8],
    limits: WireLimits,
) -> Result<(u64, R), WireCodecError>
where
    R: WireRequest + DeserializeOwned,
{
    let frame = decode_binary_frame(bytes, limits)?;
    validate_request_frame::<R>(&frame)?;
    let request = decode_binary_payload::<R>(&frame)?;
    request.validate(limits)?;
    Ok((frame.header.request_id, request))
}

pub fn decode_binary_any_request(
    bytes: &[u8],
    limits: WireLimits,
) -> Result<DecodedWireRequest, WireCodecError> {
    let frame = decode_binary_frame(bytes, limits)?;
    if !frame.header.flags.contains(WireFlags::REQUEST) {
        return Err(WireCodecError::InvalidFlags(frame.header.flags.bits()));
    }
    let request = decode_any_wire_request!(frame.header.opcode, decode_binary_payload, &frame);
    request.validate(limits)?;
    Ok(DecodedWireRequest {
        request_id: frame.header.request_id,
        request,
    })
}

#[expect(
    clippy::result_large_err,
    reason = "the public wire API returns the structured RuntimeError contract unchanged"
)]
pub fn decode_binary_response<R: WireRequest>(
    bytes: &[u8],
    expected_request_id: u64,
    limits: WireLimits,
) -> Result<R::Response, RuntimeError>
where
    R::Response: DeserializeOwned,
{
    let frame = decode_binary_frame(bytes, limits).map_err(wire_runtime_error)?;
    if !frame.header.flags.contains(WireFlags::RESPONSE)
        || frame.header.opcode != R::OPCODE
        || frame.header.request_id != expected_request_id
    {
        return Err(wire_runtime_error(WireCodecError::ResponseMismatch));
    }
    if frame.header.flags.contains(WireFlags::ERROR) {
        let error = decode_binary_payload::<RuntimeError>(&frame).map_err(wire_runtime_error)?;
        return Err(error);
    }
    decode_binary_payload::<R::Response>(&frame).map_err(wire_runtime_error)
}

fn validate_request_frame<R: WireRequest>(frame: &BinaryFrame) -> Result<(), WireCodecError> {
    if !frame.header.flags.contains(WireFlags::REQUEST) || frame.header.opcode != R::OPCODE {
        return Err(WireCodecError::ResponseMismatch);
    }
    Ok(())
}

fn wire_runtime_error(error: WireCodecError) -> RuntimeError {
    let mut runtime = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "runtime_wire.binary",
        "wire.binary.decode",
    );
    runtime.evidence.insert(
        "reason".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    runtime
}
