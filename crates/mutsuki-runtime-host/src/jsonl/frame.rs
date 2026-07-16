use std::io::BufRead;

use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_wire::JsonlResponseEnvelope;

use crate::multiplexer::{FrameCodec, transport_failure};

#[derive(Clone, Copy)]
pub(super) struct JsonlFrameCodec {
    max_line_bytes: usize,
}

impl JsonlFrameCodec {
    pub(super) fn new(max_line_bytes: usize) -> Self {
        Self { max_line_bytes }
    }
}

impl FrameCodec for JsonlFrameCodec {
    fn read_frame<R: BufRead>(&self, reader: &mut R) -> RuntimeResult<Option<Vec<u8>>> {
        read_bounded_line(reader, self.max_line_bytes)
    }

    fn response_id(&self, frame: &[u8]) -> RuntimeResult<u64> {
        serde_json::from_slice::<JsonlResponseEnvelope>(frame)
            .map(|response| response.request_id)
            .map_err(|error| transport_failure(&format!("malformed JSONL response: {error}")))
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> RuntimeResult<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| transport_failure(&format!("reader failure: {error}")))?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(transport_failure("truncated JSONL frame"))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(consumed) > limit {
            return Err(transport_failure(
                "JSONL frame exceeds configured line limit",
            ));
        }
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}
