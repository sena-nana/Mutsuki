use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use mutsuki_runtime_contracts::{CompletionBatch, EntryCompletion, RunnerResult};
use mutsuki_runtime_wire::{
    AnyWireRequest, DEBUG_JSONL_CODEC_ID, DEFAULT_WIRE_LIMITS, Opcode, RunBatchRequest,
    decode_jsonl_any_request, encode_jsonl_response,
};
use serde::Serialize;

use super::io::{ChannelReader, ChannelWriter};

#[derive(Clone, Copy)]
pub(super) enum ServerMode {
    Cancel,
    Concurrent { group_size: usize },
}

pub(super) struct ServerHandle {
    runs: Arc<(Mutex<usize>, Condvar)>,
    thread: JoinHandle<Result<(), String>>,
}

impl ServerHandle {
    pub(super) fn wait_for_runs(&self, expected: usize) -> Result<(), String> {
        let (runs, wake) = &*self.runs;
        let (runs, timeout) = wake
            .wait_timeout_while(
                runs.lock().map_err(|error| error.to_string())?,
                Duration::from_secs(2),
                |runs| *runs < expected,
            )
            .map_err(|error| error.to_string())?;
        if timeout.timed_out() || *runs < expected {
            return Err(format!("server observed {} of {expected} runs", *runs));
        }
        Ok(())
    }

    pub(super) fn join(self) -> Result<(), String> {
        self.thread
            .join()
            .map_err(|_| "benchmark server panicked".to_string())?
    }
}

pub(super) fn spawn(mode: ServerMode) -> (ChannelReader, ChannelWriter, ServerHandle) {
    let (request_sender, request_receiver) = mpsc::channel::<Vec<u8>>();
    let (response_sender, response_receiver) = mpsc::channel::<Vec<u8>>();
    let runs = Arc::new((Mutex::new(0), Condvar::new()));
    let server_runs = runs.clone();
    let thread = std::thread::Builder::new()
        .name("mutsuki-wire-p1-benchmark-server".into())
        .spawn(move || {
            let mut pending_runs = Vec::new();
            while let Ok(frame) = request_receiver.recv() {
                let decoded = decode_jsonl_any_request(&frame, DEFAULT_WIRE_LIMITS)
                    .map_err(|error| error.to_string())?;
                let request_id = decoded.request_id;
                match decoded.request {
                    AnyWireRequest::Initialize(request) => {
                        let ack = request
                            .hello
                            .accept(DEBUG_JSONL_CODEC_ID, None)
                            .map_err(|error| error.to_string())?;
                        send_response(
                            &response_sender,
                            request_id,
                            Opcode::PluginInitialize,
                            &ack,
                        )?;
                    }
                    AnyWireRequest::RunBatch(request) => {
                        let completion = completion(&request);
                        let (count, wake) = &*server_runs;
                        *count.lock().map_err(|error| error.to_string())? += 1;
                        wake.notify_all();
                        pending_runs.push((request_id, completion));
                        if let ServerMode::Concurrent { group_size } = mode
                            && pending_runs.len() == group_size
                        {
                            for (id, completion) in pending_runs.drain(..).rev() {
                                send_response(
                                    &response_sender,
                                    id,
                                    Opcode::RunnerRunBatch,
                                    &completion,
                                )?;
                            }
                        }
                    }
                    AnyWireRequest::CancelRunner(_) => {
                        send_response(&response_sender, request_id, Opcode::RunnerCancel, &())?;
                        if matches!(mode, ServerMode::Cancel) {
                            for (id, completion) in pending_runs.drain(..) {
                                send_response(
                                    &response_sender,
                                    id,
                                    Opcode::RunnerRunBatch,
                                    &completion,
                                )?;
                            }
                        }
                    }
                    AnyWireRequest::DisposeRunner(_) => {
                        send_response(&response_sender, request_id, Opcode::RunnerDispose, &())?;
                    }
                    other => {
                        return Err(format!("unexpected opcode {:#06x}", other.opcode() as u16));
                    }
                }
            }
            Ok(())
        })
        .expect("spawn P1 benchmark server");
    (
        ChannelReader::new(response_receiver),
        ChannelWriter::new(request_sender),
        ServerHandle { runs, thread },
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

fn send_response<T: Serialize>(
    sender: &mpsc::Sender<Vec<u8>>,
    request_id: u64,
    opcode: Opcode,
    value: &T,
) -> Result<(), String> {
    let frame = encode_jsonl_response(request_id, opcode, Ok(value), DEFAULT_WIRE_LIMITS)
        .map_err(|error| error.to_string())?;
    sender.send(frame).map_err(|error| error.to_string())
}
