#![no_main]

use libfuzzer_sys::fuzz_target;
use mutsuki_runtime_wire::{
    BinaryFrame, DEFAULT_WIRE_LIMITS, Opcode, WireFlags, WireHeader, WireProtocolVersion,
    decode_binary_any_request, decode_binary_frame, decode_binary_payload,
};

fuzz_target!(|data: &[u8]| {
    let payload = BinaryFrame {
        header: WireHeader {
            protocol: WireProtocolVersion::CURRENT,
            opcode: Opcode::PluginInitialize,
            flags: WireFlags::REQUEST,
            request_id: 1,
            payload_len: data.len() as u32,
        },
        payload: data.to_vec(),
    };
    let _ = decode_binary_payload::<serde_json::Value>(&payload);
    if let Ok(frame) = decode_binary_frame(data, DEFAULT_WIRE_LIMITS) {
        let _ = decode_binary_payload::<serde_json::Value>(&frame);
        let _ = decode_binary_any_request(data, DEFAULT_WIRE_LIMITS);
    }
});
