use std::collections::BTreeSet;
use std::io::Cursor;

use serde_json::{Value, json};

use crate::*;

#[test]
fn published_opcodes_are_unique_and_schema_is_generated_from_registry() {
    let values = Opcode::ALL
        .into_iter()
        .map(|opcode| opcode as u16)
        .collect::<BTreeSet<_>>();
    assert_eq!(values.len(), Opcode::ALL.len());

    let checked_in: Value =
        serde_json::from_str(include_str!("../schema/runtime-wire-v1.json")).unwrap();
    assert_eq!(checked_in, generated_schema_value());

    let fixtures: Value =
        serde_json::from_str(include_str!("../schema/runtime-wire-fixtures-v1.json")).unwrap();
    assert_eq!(fixtures, generated_fixtures_value());
}

#[test]
fn typed_jsonl_accepts_additive_fields_and_rejects_breaking_major_at_decode() {
    let request = CancelRunnerRequest {
        runner_id: "runner-a".into(),
        invocation_id: "inv-1".into(),
    };
    let encoded = encode_jsonl_request(7, &request, DEFAULT_WIRE_LIMITS).unwrap();
    let mut envelope: Value = serde_json::from_slice(&encoded).unwrap();
    envelope["payload"]["future_additive"] = Value::Bool(true);
    envelope["payload_len"] =
        Value::from(serde_json::to_vec(&envelope["payload"]).unwrap().len() as u64);
    let encoded = serde_json::to_vec(&envelope).unwrap();
    let (_, decoded) =
        decode_jsonl_request::<CancelRunnerRequest>(&encoded, DEFAULT_WIRE_LIMITS).unwrap();
    assert_eq!(decoded, request);

    envelope["protocol"]["major"] = Value::from(2);
    let error = decode_jsonl_request::<CancelRunnerRequest>(
        &serde_json::to_vec(&envelope).unwrap(),
        DEFAULT_WIRE_LIMITS,
    )
    .unwrap_err();
    assert!(matches!(error, WireCodecError::VersionMismatch { .. }));
}

#[test]
fn typed_jsonl_response_preserves_request_and_response_type_pairing() {
    let encoded =
        encode_jsonl_response(9, Opcode::RunnerCancel, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap();
    decode_jsonl_response::<CancelRunnerRequest>(&encoded, 9, DEFAULT_WIRE_LIMITS).unwrap();
}

#[test]
fn dynamic_typed_jsonl_rejects_unknown_opcode_and_payload_shape() {
    let request = CancelRunnerRequest {
        runner_id: "runner-a".into(),
        invocation_id: "inv-1".into(),
    };
    let encoded = encode_jsonl_request(5, &request, DEFAULT_WIRE_LIMITS).unwrap();
    let mut envelope: Value = serde_json::from_slice(&encoded).unwrap();
    envelope["opcode"] = Value::from(0x7fff_u64);
    let error =
        decode_jsonl_any_request(&serde_json::to_vec(&envelope).unwrap(), DEFAULT_WIRE_LIMITS)
            .unwrap_err();
    assert!(matches!(error, WireCodecError::UnknownOpcode(0x7fff)));

    envelope["opcode"] = Value::from(Opcode::RunnerCancel as u16);
    envelope["method"] = Value::from(Opcode::RunnerCancel.method());
    envelope["payload"] = json!({"runner_id": "runner-a"});
    envelope["payload_len"] =
        Value::from(serde_json::to_vec(&envelope["payload"]).unwrap().len() as u64);
    let error =
        decode_jsonl_any_request(&serde_json::to_vec(&envelope).unwrap(), DEFAULT_WIRE_LIMITS)
            .unwrap_err();
    assert!(matches!(error, WireCodecError::Decode(_)));
}

#[test]
fn initialization_negotiates_limits_and_rejects_missing_management_support() {
    let hello = ProtocolHello::debug_jsonl();
    let ack = hello.accept(DEBUG_JSONL_CODEC_ID, None).unwrap();
    ack.validate_for(&hello).unwrap();

    let mut incompatible = hello.clone();
    incompatible.management_channel = false;
    let error = incompatible.accept(DEBUG_JSONL_CODEC_ID, None).unwrap_err();
    assert_eq!(error, WireCodecError::ManagementChannelRequired);
}

#[test]
fn custom_limits_are_advertised_and_invalid_reservations_are_rejected() {
    let limits = WireLimits {
        max_in_flight_requests: 12,
        management_reserved_requests: 2,
        ..DEFAULT_WIRE_LIMITS
    };
    let hello = ProtocolHello::debug_jsonl_with_limits(limits).unwrap();
    assert_eq!(hello.max_in_flight_requests, 12);
    let ack = hello.accept(DEBUG_JSONL_CODEC_ID, None).unwrap();
    ack.validate_for(&hello).unwrap();

    let invalid = WireLimits {
        max_in_flight_requests: 2,
        management_reserved_requests: 2,
        ..DEFAULT_WIRE_LIMITS
    };
    assert_eq!(invalid.validate(), Err(WireCodecError::LimitMismatch));
}

#[test]
fn binary_frame_is_length_prefixed_typed_messagepack() {
    let request = DisposeRunnerRequest {
        runner_id: "runner-a".into(),
    };
    let encoded = encode_binary_request(11, &request, DEFAULT_WIRE_LIMITS).unwrap();
    let frame = decode_binary_frame(&encoded, DEFAULT_WIRE_LIMITS).unwrap();
    assert_eq!(frame.header.opcode, Opcode::RunnerDispose);
    assert!(frame.header.flags.contains(WireFlags::REQUEST));
    assert!(frame.header.flags.contains(WireFlags::MANAGEMENT));
    assert_eq!(
        decode_binary_payload::<DisposeRunnerRequest>(&frame).unwrap(),
        request
    );

    let from_reader = read_binary_frame(&mut Cursor::new(encoded), DEFAULT_WIRE_LIMITS).unwrap();
    assert_eq!(from_reader, frame);
}

#[test]
fn binary_reader_rejects_oversized_prefix_before_payload_allocation() {
    let declared = (DEFAULT_WIRE_LIMITS.max_frame_bytes as u32 + 1).to_be_bytes();
    let error = read_binary_frame(&mut Cursor::new(declared), DEFAULT_WIRE_LIMITS).unwrap_err();
    assert!(matches!(error, WireCodecError::FrameOversized { .. }));
}

#[test]
fn large_resource_bytes_must_use_resource_ref_or_stream() {
    let request = CreateBlobRequest {
        provider_id: Some("provider-a".into()),
        schema: "bytes.v1".into(),
        bytes: vec![0; DEFAULT_WIRE_LIMITS.max_inline_resource_bytes + 1],
    };
    let error = encode_binary_request(12, &request, DEFAULT_WIRE_LIMITS).unwrap_err();
    assert!(matches!(
        error,
        WireCodecError::InlineResourceOversized { .. }
    ));
}
