use std::sync::mpsc;
use std::thread::JoinHandle;

use mutsuki_runtime_contracts::{CompletionBatch, EntryCompletion, RunnerResult};
use mutsuki_runtime_wire::{
    AnyWireRequest, BINARY_CODEC_ID, DEBUG_JSONL_CODEC_ID, DEFAULT_WIRE_LIMITS, Opcode,
    RunBatchRequest, decode_binary_any_request, decode_jsonl_any_request, encode_binary_response,
    encode_jsonl_response,
};
use serde::Serialize;

use super::io::{ChannelReader, ChannelWriter};

#[derive(Clone, Copy)]
pub(super) enum Codec {
    Jsonl,
    Binary,
}

pub(super) struct ServerHandle(JoinHandle<Result<(), String>>);

impl ServerHandle {
    pub(super) fn join(self) -> Result<(), String> {
        self.0
            .join()
            .map_err(|_| "P2 benchmark server panicked".to_string())?
    }
}

pub(super) fn spawn(
    codec: Codec,
    group_size: usize,
) -> (ChannelReader, ChannelWriter, ServerHandle) {
    let (request_sender, request_receiver) = mpsc::channel::<Vec<u8>>();
    let (response_sender, response_receiver) = mpsc::channel::<Vec<u8>>();
    let thread = std::thread::Builder::new()
        .name("mutsuki-wire-p2-benchmark-server".into())
        .spawn(move || {
            let mut pending = Vec::new();
            while let Ok(frame) = request_receiver.recv() {
                let decoded = match codec {
                    Codec::Jsonl => decode_jsonl_any_request(&frame, DEFAULT_WIRE_LIMITS),
                    Codec::Binary => decode_binary_any_request(&frame, DEFAULT_WIRE_LIMITS),
                }
                .map_err(|error| error.to_string())?;
                let request_id = decoded.request_id;
                match decoded.request {
                    AnyWireRequest::Initialize(request) => {
                        let codec_id = match codec {
                            Codec::Jsonl => DEBUG_JSONL_CODEC_ID,
                            Codec::Binary => BINARY_CODEC_ID,
                        };
                        let ack = request
                            .hello
                            .accept(codec_id, None)
                            .map_err(|error| error.to_string())?;
                        send(
                            &response_sender,
                            codec,
                            request_id,
                            Opcode::PluginInitialize,
                            &ack,
                        )?;
                    }
                    AnyWireRequest::RunBatch(request) => {
                        pending.push((request_id, completion(&request)));
                        if pending.len() == group_size {
                            for (id, value) in pending.drain(..).rev() {
                                send(&response_sender, codec, id, Opcode::RunnerRunBatch, &value)?;
                            }
                        }
                    }
                    other => {
                        return Err(format!("unexpected opcode {:#06x}", other.opcode() as u16));
                    }
                }
            }
            Ok(())
        })
        .expect("spawn P2 benchmark server");
    (
        ChannelReader::new(response_receiver),
        ChannelWriter::new(request_sender),
        ServerHandle(thread),
    )
}

fn completion(request: &RunBatchRequest) -> CompletionBatch {
    CompletionBatch {
        batch_id: request.batch.batch_id.clone(),
        tick_id: request.batch.tick_id.clone(),
        results: request
            .batch
            .entries
            .iter()
            .map(|entry| EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: Some(RunnerResult::completed(&entry.task_id)),
                error: None,
            })
            .collect(),
        metadata: Vec::new(),
    }
}

fn send<T: Serialize>(
    sender: &mpsc::Sender<Vec<u8>>,
    codec: Codec,
    request_id: u64,
    opcode: Opcode,
    value: &T,
) -> Result<(), String> {
    let frame = match codec {
        Codec::Jsonl => encode_jsonl_response(request_id, opcode, Ok(value), DEFAULT_WIRE_LIMITS),
        Codec::Binary => encode_binary_response(request_id, opcode, Ok(value), DEFAULT_WIRE_LIMITS),
    }
    .map_err(|error| error.to_string())?;
    sender.send(frame).map_err(|error| error.to_string())
}
