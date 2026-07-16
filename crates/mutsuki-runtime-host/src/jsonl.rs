use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, RunnerDescriptor, RuntimeError,
    ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::{
    CancelRunnerRequest, CommandBatchRequest, CommandPlanRequest, DEFAULT_WIRE_LIMITS,
    DisposeRunnerRequest, ExportPlanRequest, InitializeRequest, JsonlResponseEnvelope,
    ProtocolHello, RunBatchRequest, SagaPlanRequest, WireLimits, WireRequest,
    decode_jsonl_response, encode_jsonl_request,
};

const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct JsonlRunner<R, W> {
    descriptor: RunnerDescriptor,
    bridge: JsonlBridge<R, W>,
}

pub(crate) struct JsonlBridge<R, W> {
    shared: Arc<BridgeShared<W>>,
    reader: Mutex<ReaderState<R>>,
    handshake: Mutex<Option<Result<(), RuntimeError>>>,
    limits: WireLimits,
    response_timeout: Duration,
}

struct BridgeShared<W> {
    writer: Mutex<W>,
    next_request: AtomicU64,
    pending: Arc<Mutex<PendingState>>,
    pending_ready: Arc<Condvar>,
}

struct PendingState {
    requests: BTreeMap<u64, SyncSender<RuntimeResult<Vec<u8>>>>,
    failure: Option<RuntimeError>,
    closed: bool,
}

struct ReaderState<R> {
    reader: Option<R>,
    thread: Option<JoinHandle<R>>,
}

impl<R, W> JsonlBridge<R, W> {
    pub(crate) fn new(reader: R, writer: W) -> Self {
        Self::with_limits(
            reader,
            writer,
            DEFAULT_WIRE_LIMITS,
            DEFAULT_RESPONSE_TIMEOUT,
        )
    }

    pub(crate) fn with_limits(
        reader: R,
        writer: W,
        limits: WireLimits,
        response_timeout: Duration,
    ) -> Self {
        Self {
            shared: Arc::new(BridgeShared {
                writer: Mutex::new(writer),
                next_request: AtomicU64::new(0),
                pending: Arc::new(Mutex::new(PendingState {
                    requests: BTreeMap::new(),
                    failure: None,
                    closed: false,
                })),
                pending_ready: Arc::new(Condvar::new()),
            }),
            reader: Mutex::new(ReaderState {
                reader: Some(reader),
                thread: None,
            }),
            handshake: Mutex::new(None),
            limits,
            response_timeout,
        }
    }

    pub(crate) fn into_inner(self) -> (R, W) {
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.closed = true;
            self.shared.pending_ready.notify_all();
        }
        let mut reader_state = self
            .reader
            .into_inner()
            .expect("jsonl bridge reader mutex poisoned");
        let reader = if let Some(reader) = reader_state.reader.take() {
            reader
        } else {
            reader_state
                .thread
                .take()
                .expect("jsonl bridge reader thread missing")
                .join()
                .expect("jsonl bridge reader thread panicked")
        };
        let shared = Arc::try_unwrap(self.shared)
            .unwrap_or_else(|_| panic!("jsonl bridge still has active reader state"));
        let writer = shared
            .writer
            .into_inner()
            .expect("jsonl bridge writer mutex poisoned");
        (reader, writer)
    }
}

impl<R, W> JsonlRunner<R, W> {
    pub fn new(descriptor: RunnerDescriptor, reader: R, writer: W) -> Self {
        Self {
            descriptor,
            bridge: JsonlBridge::new(reader, writer),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.bridge.into_inner()
    }
}

impl<R, W> JsonlBridge<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub(crate) fn request<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        if T::OPCODE != mutsuki_runtime_wire::Opcode::PluginInitialize {
            self.ensure_initialized()?;
        }
        self.request_without_handshake(request)
    }

    fn ensure_initialized(&self) -> RuntimeResult<()> {
        let mut handshake = self
            .handshake
            .lock()
            .map_err(|_| protocol_error("handshake lock poisoned"))?;
        if let Some(result) = handshake.as_ref() {
            return result.clone().map_err(RuntimeFailure::new);
        }
        let hello = ProtocolHello::debug_jsonl();
        let result = self
            .request_without_handshake(&InitializeRequest {
                hello: hello.clone(),
            })
            .and_then(|ack| {
                ack.validate_for(&hello)
                    .map_err(|error| protocol_error(&error.to_string()))
            });
        *handshake = Some(match &result {
            Ok(()) => Ok(()),
            Err(error) => Err(error.error().clone()),
        });
        result
    }

    fn request_without_handshake<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        let request_id = self.shared.next_request.fetch_add(1, Ordering::Relaxed) + 1;
        let encoded = encode_jsonl_request(request_id, request, self.limits)
            .map_err(|error| protocol_error(&error.to_string()))?;
        let (sender, receiver) = mpsc::sync_channel(1);
        self.insert_pending(request_id, T::OPCODE.is_management(), sender)?;
        if let Err(error) = self.ensure_reader_started() {
            self.remove_pending(request_id);
            return Err(error);
        }
        if let Err(error) = self.write_frame(&encoded) {
            self.remove_pending(request_id);
            fail_transport(&self.shared.pending, error.error().clone());
            return Err(error);
        }
        let line = receiver
            .recv_timeout(self.response_timeout)
            .map_err(|error| {
                self.remove_pending(request_id);
                protocol_error(&format!("response timeout or disconnect: {error}"))
            })??;
        decode_jsonl_response::<T>(&line, request_id, self.limits).map_err(RuntimeFailure::new)
    }

    fn insert_pending(
        &self,
        request_id: u64,
        management: bool,
        sender: SyncSender<RuntimeResult<Vec<u8>>>,
    ) -> RuntimeResult<()> {
        let mut pending = self
            .shared
            .pending
            .lock()
            .map_err(|_| protocol_error("pending table lock poisoned"))?;
        if let Some(error) = pending.failure.clone() {
            return Err(RuntimeFailure::new(error));
        }
        let capacity = if management {
            self.limits.max_in_flight_requests
        } else {
            self.limits
                .max_in_flight_requests
                .saturating_sub(self.limits.management_reserved_requests)
        };
        if pending.requests.len() >= capacity {
            return Err(protocol_error("pending request limit reached"));
        }
        if pending.requests.insert(request_id, sender).is_some() {
            return Err(protocol_error("duplicate request id"));
        }
        self.shared.pending_ready.notify_one();
        Ok(())
    }

    fn remove_pending(&self, request_id: u64) {
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.requests.remove(&request_id);
        }
    }

    fn ensure_reader_started(&self) -> RuntimeResult<()> {
        let mut state = self
            .reader
            .lock()
            .map_err(|_| protocol_error("reader state lock poisoned"))?;
        if state.thread.is_some() {
            return Ok(());
        }
        let reader = state
            .reader
            .take()
            .ok_or_else(|| protocol_error("reader unavailable"))?;
        let pending = Arc::clone(&self.shared.pending);
        let pending_ready = Arc::clone(&self.shared.pending_ready);
        let limit = self.limits.max_jsonl_line_bytes;
        state.thread = Some(
            thread::Builder::new()
                .name("mutsuki-jsonl-reader".into())
                .spawn(move || reader_loop(reader, pending, pending_ready, limit))
                .map_err(|error| protocol_error(&error.to_string()))?,
        );
        Ok(())
    }

    fn write_frame(&self, encoded: &[u8]) -> RuntimeResult<()> {
        let mut writer = self
            .shared
            .writer
            .lock()
            .map_err(|_| protocol_error("writer lock poisoned"))?;
        writer.write_all(encoded).map_err(io_error_failure)?;
        writer.flush().map_err(io_error_failure)
    }
}

fn reader_loop<R: BufRead>(
    mut reader: R,
    pending_mutex: Arc<Mutex<PendingState>>,
    pending_ready: Arc<Condvar>,
    max_line_bytes: usize,
) -> R {
    loop {
        let mut pending = match pending_mutex.lock() {
            Ok(pending) => pending,
            Err(_) => return reader,
        };
        while pending.requests.is_empty() && !pending.closed {
            pending = match pending_ready.wait(pending) {
                Ok(pending) => pending,
                Err(_) => return reader,
            };
        }
        if pending.closed {
            return reader;
        }
        drop(pending);
        match read_bounded_line(&mut reader, max_line_bytes) {
            Ok(Some(line)) => {
                let response = serde_json::from_slice::<JsonlResponseEnvelope>(&line);
                match response {
                    Ok(response) if response.request_id != 0 => {
                        let sender = match pending_mutex.lock() {
                            Ok(mut pending) => pending.requests.remove(&response.request_id),
                            Err(_) => return reader,
                        };
                        match sender {
                            Some(sender) => {
                                let _ = sender.send(Ok(line));
                            }
                            None => {
                                fail_transport(
                                    &pending_mutex,
                                    protocol_error_value("unknown or late response id"),
                                );
                                return reader;
                            }
                        }
                    }
                    Ok(_) => {
                        fail_transport(&pending_mutex, protocol_error_value("zero response id"));
                        return reader;
                    }
                    Err(error) => {
                        fail_transport(
                            &pending_mutex,
                            protocol_error_value(&format!("malformed response: {error}")),
                        );
                        return reader;
                    }
                }
            }
            Ok(None) => {
                fail_transport(&pending_mutex, protocol_error_value("unexpected EOF"));
                return reader;
            }
            Err(error) => {
                fail_transport(&pending_mutex, error.error().clone());
                return reader;
            }
        }
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> RuntimeResult<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(io_error_failure)?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(protocol_error("truncated JSONL frame"))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(consumed) > limit {
            return Err(protocol_error("JSONL frame exceeds configured line limit"));
        }
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

fn fail_transport(pending_mutex: &Mutex<PendingState>, error: RuntimeError) {
    let senders = match pending_mutex.lock() {
        Ok(mut pending) => {
            if pending.failure.is_none() {
                pending.failure = Some(error.clone());
            }
            std::mem::take(&mut pending.requests)
        }
        Err(_) => return,
    };
    for (_, sender) in senders {
        let _ = sender.send(Err(RuntimeFailure::new(error.clone())));
    }
}

impl<R, W> JsonlRunner<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.bridge.request(&ExportPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    pub fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.bridge.request(&CommandPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    pub fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.bridge.request(&CommandBatchRequest {
            provider_id: None,
            batch: batch.clone(),
        })
    }

    pub fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.bridge.request(&SagaPlanRequest {
            provider_id: None,
            saga: saga.clone(),
        })
    }
}

impl<R, W> Runner for JsonlRunner<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        validate_batch_leases(&ctx, &batch)?;
        self.bridge.request(&RunBatchRequest {
            runner_id: self.descriptor.runner_id.clone(),
            ctx,
            batch,
        })
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.bridge.request(&CancelRunnerRequest {
            runner_id: self.descriptor.runner_id.clone(),
            invocation_id: invocation_id.into(),
        })
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.bridge.request(&DisposeRunnerRequest {
            runner_id: self.descriptor.runner_id.clone(),
        })
    }
}

fn validate_batch_leases(ctx: &RunnerContext, batch: &WorkBatch) -> RuntimeResult<()> {
    let batch_lease_ids = batch
        .task_leases
        .iter()
        .map(|lease| lease.lease_id.clone())
        .collect::<Vec<_>>();
    if batch_lease_ids != ctx.task_lease_ids {
        let mut error = RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
            "jsonl_runner",
            format!("runner.run_batch.{}", batch.batch_id),
        );
        error.evidence.insert(
            "ctx_task_lease_ids".into(),
            ScalarValue::String(ctx.task_lease_ids.join(",")),
        );
        error.evidence.insert(
            "batch_task_lease_ids".into(),
            ScalarValue::String(batch_lease_ids.join(",")),
        );
        error.evidence.insert(
            "executor_id".into(),
            ScalarValue::String(ctx.executor_id.clone()),
        );
        return Err(RuntimeFailure::new(error));
    }
    Ok(())
}

fn io_error_failure(error: std::io::Error) -> RuntimeFailure {
    host_failure_with_evidence("jsonl.io", "exception_repr", error.to_string())
}

fn protocol_error(message: &str) -> RuntimeFailure {
    RuntimeFailure::new(protocol_error_value(message))
}

fn protocol_error_value(message: &str) -> RuntimeError {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "jsonl_runner",
        "jsonl.protocol",
    );
    error
        .evidence
        .insert("reason".into(), ScalarValue::String(message.into()));
    error
}

fn host_failure_with_evidence(
    route: &str,
    evidence_key: &str,
    evidence_value: impl Into<String>,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "jsonl_runner",
        route,
    );
    error.evidence.insert(
        evidence_key.into(),
        ScalarValue::String(evidence_value.into()),
    );
    RuntimeFailure::new(error)
}
