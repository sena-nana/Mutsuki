use mutsuki_runtime_contracts::{RuntimeError, ScalarValue};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::{Opcode, WireCodecError, encode_binary_response, encode_jsonl_response};
use serde::Serialize;

use super::types::ABI_CODEC_ID;

pub(crate) fn abi_failure(route: impl Into<String>, detail: impl Into<String>) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "mutsuki.runtime.abi",
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

pub(crate) fn wire_failure(error: WireCodecError) -> RuntimeFailure {
    abi_failure("abi.wire", error.to_string())
}

pub(crate) fn encode_result<T: Serialize>(
    request_id: u64,
    opcode: Opcode,
    result: RuntimeResult<T>,
) -> Vec<u8> {
    let encoded = match result {
        Ok(value) => encode_jsonl_response(
            request_id,
            opcode,
            Ok(&value),
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        ),
        Err(error) => encode_jsonl_response::<T>(
            request_id,
            opcode,
            Err(error.error()),
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        ),
    };
    encoded.unwrap_or_else(|error| {
        format!("{{\"codec\":\"{ABI_CODEC_ID}\",\"wire_error\":{error:?}}}\n").into_bytes()
    })
}

pub(crate) fn encode_binary_result<T: Serialize>(
    request_id: u64,
    opcode: Opcode,
    result: RuntimeResult<T>,
) -> Vec<u8> {
    let encoded = match result {
        Ok(value) => encode_binary_response(
            request_id,
            opcode,
            Ok(&value),
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        ),
        Err(error) => encode_binary_response::<T>(
            request_id,
            opcode,
            Err(error.error()),
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        ),
    };
    encoded.unwrap_or_default()
}
