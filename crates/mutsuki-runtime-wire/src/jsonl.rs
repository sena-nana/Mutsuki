use mutsuki_runtime_contracts::RuntimeError;
use serde::Serialize;
use serde_json::Value;

use crate::{Opcode, WireCodecError, WireLimits, WireProtocolVersion, WireRequest};

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct JsonlRequestEnvelope {
    pub request_id: u64,
    pub protocol: WireProtocolVersion,
    pub opcode: u16,
    pub method: String,
    pub payload_len: u32,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct JsonlResponseEnvelope {
    pub request_id: u64,
    pub protocol: WireProtocolVersion,
    pub opcode: u16,
    pub payload_len: u32,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<RuntimeError>,
}

pub fn encode_jsonl_request<R: WireRequest>(
    request_id: u64,
    request: &R,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    request.validate(limits)?;
    let payload =
        serde_json::to_value(request).map_err(|error| WireCodecError::Encode(error.to_string()))?;
    let payload_len = encoded_json_len(&payload)?;
    validate_payload_len(payload_len, limits)?;
    let envelope = JsonlRequestEnvelope {
        request_id,
        protocol: WireProtocolVersion::CURRENT,
        opcode: R::OPCODE as u16,
        method: R::OPCODE.method().into(),
        payload_len: payload_len as u32,
        payload,
    };
    let mut encoded =
        serde_json::to_vec(&envelope).map_err(|error| WireCodecError::Encode(error.to_string()))?;
    encoded.push(b'\n');
    if encoded.len() > limits.max_jsonl_line_bytes {
        return Err(WireCodecError::FrameOversized {
            actual: encoded.len(),
            limit: limits.max_jsonl_line_bytes,
        });
    }
    Ok(encoded)
}

pub fn decode_jsonl_request<R>(line: &[u8], limits: WireLimits) -> Result<(u64, R), WireCodecError>
where
    R: WireRequest + serde::de::DeserializeOwned,
{
    if line.len() > limits.max_jsonl_line_bytes {
        return Err(WireCodecError::FrameOversized {
            actual: line.len(),
            limit: limits.max_jsonl_line_bytes,
        });
    }
    let envelope: JsonlRequestEnvelope =
        serde_json::from_slice(line).map_err(|error| WireCodecError::Decode(error.to_string()))?;
    validate_envelope(
        envelope.request_id,
        envelope.protocol,
        envelope.opcode,
        Some(&envelope.method),
        envelope.payload_len,
        &envelope.payload,
        limits,
    )?;
    if envelope.opcode != R::OPCODE as u16 {
        return Err(WireCodecError::UnknownOpcode(envelope.opcode));
    }
    let request = serde_json::from_value(envelope.payload)
        .map_err(|error| WireCodecError::Decode(error.to_string()))?;
    Ok((envelope.request_id, request))
}

pub fn encode_jsonl_response<T: Serialize>(
    request_id: u64,
    opcode: Opcode,
    response: Result<&T, &RuntimeError>,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    let (ok, result, error) = match response {
        Ok(value) => (
            true,
            Some(
                serde_json::to_value(value)
                    .map_err(|error| WireCodecError::Encode(error.to_string()))?,
            ),
            None,
        ),
        Err(error) => (false, None, Some(error.clone())),
    };
    let payload_value = if ok {
        result.as_ref().expect("successful response result")
    } else {
        let error = error.as_ref().expect("failed response error");
        return encode_error_response(request_id, opcode, error, limits);
    };
    let payload_len = encoded_json_len(payload_value)?;
    validate_payload_len(payload_len, limits)?;
    encode_response_envelope(
        JsonlResponseEnvelope {
            request_id,
            protocol: WireProtocolVersion::CURRENT,
            opcode: opcode as u16,
            payload_len: payload_len as u32,
            ok,
            result,
            error,
        },
        limits,
    )
}

#[expect(
    clippy::result_large_err,
    reason = "the public wire API returns the structured RuntimeError contract unchanged"
)]
pub fn decode_jsonl_response<R: WireRequest>(
    line: &[u8],
    expected_request_id: u64,
    limits: WireLimits,
) -> Result<R::Response, RuntimeError> {
    decode_jsonl_response_inner::<R>(line, expected_request_id, limits)
        .map_err(wire_runtime_error)?
}

fn decode_jsonl_response_inner<R: WireRequest>(
    line: &[u8],
    expected_request_id: u64,
    limits: WireLimits,
) -> Result<Result<R::Response, RuntimeError>, WireCodecError> {
    if line.len() > limits.max_jsonl_line_bytes {
        return Err(WireCodecError::FrameOversized {
            actual: line.len(),
            limit: limits.max_jsonl_line_bytes,
        });
    }
    let envelope: JsonlResponseEnvelope =
        serde_json::from_slice(line).map_err(|error| WireCodecError::Decode(error.to_string()))?;
    if envelope.request_id != expected_request_id {
        return Err(WireCodecError::Decode(format!(
            "response request id mismatch: expected {expected_request_id}, got {}",
            envelope.request_id
        )));
    }
    if envelope.opcode != R::OPCODE as u16 {
        return Err(WireCodecError::Decode(format!(
            "response opcode mismatch: expected {:#06x}, got {:#06x}",
            R::OPCODE as u16,
            envelope.opcode
        )));
    }
    envelope.protocol.ensure_compatible()?;
    if envelope.ok {
        if envelope.error.is_some() {
            return Err(WireCodecError::Decode(
                "successful response contains error".into(),
            ));
        }
        // JSON `null` is the canonical Unit response and deserializes as `None`
        // through `Option<Value>`; preserve it as an explicit payload here.
        let result = envelope.result.unwrap_or(Value::Null);
        validate_declared_payload(envelope.payload_len, &result, limits)?;
        let decoded = serde_json::from_value(result)
            .map_err(|error| WireCodecError::Decode(error.to_string()))?;
        Ok(Ok(decoded))
    } else {
        if envelope.result.is_some() {
            return Err(WireCodecError::Decode(
                "failed response contains result".into(),
            ));
        }
        let error = envelope
            .error
            .ok_or_else(|| WireCodecError::Decode("response missing error".into()))?;
        let error_value = serde_json::to_value(&error)
            .map_err(|encode| WireCodecError::Encode(encode.to_string()))?;
        validate_declared_payload(envelope.payload_len, &error_value, limits)?;
        Ok(Err(error))
    }
}

fn encode_error_response(
    request_id: u64,
    opcode: Opcode,
    error: &RuntimeError,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    let value =
        serde_json::to_value(error).map_err(|encode| WireCodecError::Encode(encode.to_string()))?;
    let payload_len = encoded_json_len(&value)?;
    validate_payload_len(payload_len, limits)?;
    encode_response_envelope(
        JsonlResponseEnvelope {
            request_id,
            protocol: WireProtocolVersion::CURRENT,
            opcode: opcode as u16,
            payload_len: payload_len as u32,
            ok: false,
            result: None,
            error: Some(error.clone()),
        },
        limits,
    )
}

fn encode_response_envelope(
    envelope: JsonlResponseEnvelope,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    let mut encoded =
        serde_json::to_vec(&envelope).map_err(|error| WireCodecError::Encode(error.to_string()))?;
    encoded.push(b'\n');
    if encoded.len() > limits.max_jsonl_line_bytes {
        return Err(WireCodecError::FrameOversized {
            actual: encoded.len(),
            limit: limits.max_jsonl_line_bytes,
        });
    }
    Ok(encoded)
}

fn validate_envelope(
    request_id: u64,
    protocol: WireProtocolVersion,
    opcode: u16,
    method: Option<&str>,
    payload_len: u32,
    payload: &Value,
    limits: WireLimits,
) -> Result<(), WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    protocol.ensure_compatible()?;
    let opcode = Opcode::from_u16(opcode)?;
    if let Some(method) = method
        && method != opcode.method()
    {
        return Err(WireCodecError::MethodMismatch {
            opcode: opcode as u16,
            expected: opcode.method().into(),
            actual: method.into(),
        });
    }
    validate_declared_payload(payload_len, payload, limits)
}

fn validate_declared_payload(
    declared: u32,
    payload: &Value,
    limits: WireLimits,
) -> Result<(), WireCodecError> {
    let actual = encoded_json_len(payload)?;
    validate_payload_len(actual, limits)?;
    if declared as usize != actual {
        return Err(WireCodecError::PayloadLengthMismatch {
            declared: declared as usize,
            actual,
        });
    }
    Ok(())
}

fn validate_payload_len(length: usize, limits: WireLimits) -> Result<(), WireCodecError> {
    if length > limits.max_payload_bytes {
        return Err(WireCodecError::PayloadOversized {
            actual: length,
            limit: limits.max_payload_bytes,
        });
    }
    Ok(())
}

fn encoded_json_len(value: &Value) -> Result<usize, WireCodecError> {
    serde_json::to_vec(value)
        .map(|encoded| encoded.len())
        .map_err(|error| WireCodecError::Encode(error.to_string()))
}

fn wire_runtime_error(error: WireCodecError) -> RuntimeError {
    let mut runtime_error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "mutsuki.runtime.wire",
        "wire.jsonl.decode_response",
    );
    runtime_error.evidence.insert(
        "wire_error".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    runtime_error
}
