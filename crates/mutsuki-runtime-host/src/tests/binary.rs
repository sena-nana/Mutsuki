use std::io::Cursor;

use mutsuki_runtime_wire::{
    BINARY_CODEC_ID, DEFAULT_WIRE_LIMITS, DisposeRunnerRequest, InitializeRequest, Opcode,
    ProtocolHello, decode_binary_request, encode_binary_response,
};
use serde_json::json;

use crate::BinaryTransport;

#[test]
fn binary_stdio_transport_reuses_typed_handshake_and_multiplexer() {
    let hello = ProtocolHello::binary();
    let ack = hello.accept(BINARY_CODEC_ID, None).unwrap();
    let mut responses =
        encode_binary_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS).unwrap();
    responses.extend(
        encode_binary_response(2, Opcode::RunnerDispose, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap(),
    );
    let transport = BinaryTransport::new(Cursor::new(responses), Cursor::new(Vec::<u8>::new()));

    transport
        .request(&DisposeRunnerRequest {
            runner_id: "binary.runner".into(),
        })
        .unwrap();

    let (_, written) = transport.into_inner();
    let bytes = written.into_inner();
    let first_len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize + 4;
    assert_eq!(
        mutsuki_runtime_wire::decode_binary_frame(&bytes[..first_len], DEFAULT_WIRE_LIMITS)
            .unwrap()
            .header
            .opcode,
        Opcode::PluginInitialize
    );
}

#[test]
fn explicit_binary_initialize_records_configured_handshake_once() {
    let hello = ProtocolHello::binary();
    let ack = hello.accept(BINARY_CODEC_ID, None).unwrap();
    let mut responses =
        encode_binary_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS).unwrap();
    responses.extend(
        encode_binary_response(2, Opcode::RunnerDispose, Ok(&()), DEFAULT_WIRE_LIMITS).unwrap(),
    );
    let transport = BinaryTransport::new(Cursor::new(responses), Cursor::new(Vec::<u8>::new()));

    transport
        .initialize(Some(json!({"configured": true})))
        .unwrap();
    transport
        .request(&DisposeRunnerRequest {
            runner_id: "binary.runner".into(),
        })
        .unwrap();

    let (_, written) = transport.into_inner();
    let bytes = written.into_inner();
    let first_len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize + 4;
    let (_, initialize) =
        decode_binary_request::<InitializeRequest>(&bytes[..first_len], DEFAULT_WIRE_LIMITS)
            .unwrap();
    assert_eq!(initialize.config, Some(json!({"configured": true})));
    assert_eq!(
        bytes.len(),
        first_len
            + u32::from_be_bytes(bytes[first_len..first_len + 4].try_into().unwrap()) as usize
            + 4
    );
}
