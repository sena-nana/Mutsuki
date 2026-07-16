use std::io::BufRead;

use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_wire::{WireFlags, WireLimits, decode_binary_frame, read_binary_frame_bytes};

use crate::multiplexer::{FrameCodec, transport_failure};

#[derive(Clone, Copy)]
pub(super) struct BinaryFrameCodec {
    limits: WireLimits,
}

impl BinaryFrameCodec {
    pub(super) fn new(limits: WireLimits) -> Self {
        Self { limits }
    }
}

impl FrameCodec for BinaryFrameCodec {
    fn read_frame<R: BufRead>(&self, reader: &mut R) -> RuntimeResult<Option<Vec<u8>>> {
        read_binary_frame_bytes(reader, self.limits)
            .map_err(|error| transport_failure(&error.to_string()))
    }

    fn response_id(&self, frame: &[u8]) -> RuntimeResult<u64> {
        let decoded = decode_binary_frame(frame, self.limits)
            .map_err(|error| transport_failure(&error.to_string()))?;
        if !decoded.header.flags.contains(WireFlags::RESPONSE) {
            return Err(transport_failure(
                "binary peer emitted a non-response frame",
            ));
        }
        Ok(decoded.header.request_id)
    }
}
