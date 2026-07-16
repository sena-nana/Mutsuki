mod frame;

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, RunnerDescriptor, RuntimeError,
    ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{
    Runner, RunnerContext, RunnerManagementHandle, RuntimeFailure, RuntimeResult,
};
use mutsuki_runtime_wire::{
    CancelRunnerRequest, CommandBatchRequest, CommandPlanRequest, DEFAULT_WIRE_LIMITS,
    DisposeRunnerRequest, ExportPlanRequest, InitializeRequest, ProtocolHello, RunBatchRequest,
    SagaPlanRequest, WireLimits, WireRequest, decode_jsonl_response, encode_jsonl_request,
};

use self::frame::JsonlFrameCodec;
use crate::multiplexer::{RequestMultiplexer, transport_failure};

const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct JsonlRunner<R, W> {
    descriptor: RunnerDescriptor,
    transport: JsonlTransport<R, W>,
}

pub struct JsonlTransport<R, W> {
    bridge: Arc<JsonlBridge<R, W>>,
}

struct JsonlBridge<R, W> {
    multiplexer: RequestMultiplexer<R, W>,
    handshake: Mutex<Option<Result<(), RuntimeError>>>,
    limits: Mutex<WireLimits>,
}

struct JsonlManagement<R, W> {
    runner_id: String,
    transport: JsonlTransport<R, W>,
}

impl<R, W> std::fmt::Debug for JsonlManagement<R, W> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JsonlManagement")
            .field("runner_id", &self.runner_id)
            .finish_non_exhaustive()
    }
}

impl<R, W> RunnerManagementHandle for JsonlManagement<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    fn cancel(&self, invocation_id: &str) -> RuntimeResult<()> {
        self.transport.request(&CancelRunnerRequest {
            runner_id: self.runner_id.clone(),
            invocation_id: invocation_id.into(),
        })
    }

    fn dispose(&self) -> RuntimeResult<()> {
        self.transport.request(&DisposeRunnerRequest {
            runner_id: self.runner_id.clone(),
        })
    }
}

impl<R, W> Clone for JsonlTransport<R, W> {
    fn clone(&self) -> Self {
        Self {
            bridge: self.bridge.clone(),
        }
    }
}

impl<R, W> JsonlRunner<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn new(descriptor: RunnerDescriptor, reader: R, writer: W) -> Self {
        Self {
            descriptor,
            transport: JsonlTransport::new(reader, writer),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.transport.into_inner()
    }

    pub fn transport(&self) -> JsonlTransport<R, W> {
        self.transport.clone()
    }
}

impl<R, W> JsonlTransport<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self::with_limits(
            reader,
            writer,
            DEFAULT_WIRE_LIMITS,
            DEFAULT_RESPONSE_TIMEOUT,
        )
        .expect("default Runtime Wire limits must be valid")
    }

    pub fn with_limits(
        reader: R,
        writer: W,
        limits: WireLimits,
        response_timeout: Duration,
    ) -> RuntimeResult<Self> {
        limits
            .validate()
            .map_err(|error| transport_failure(&error.to_string()))?;
        if response_timeout.is_zero() {
            return Err(transport_failure(
                "response timeout must be greater than zero",
            ));
        }
        Ok(Self {
            bridge: Arc::new(JsonlBridge {
                multiplexer: RequestMultiplexer::new(
                    reader,
                    writer,
                    JsonlFrameCodec::new(limits.max_jsonl_line_bytes),
                    limits,
                    response_timeout,
                ),
                handshake: Mutex::new(None),
                limits: Mutex::new(limits),
            }),
        })
    }

    pub fn into_inner(self) -> (R, W) {
        match Arc::try_unwrap(self.bridge) {
            Ok(bridge) => bridge.multiplexer.into_inner(),
            Err(_) => panic!("JSONL transport still has active handles"),
        }
    }

    pub fn request<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        self.bridge.request(request)
    }
}

impl<R, W> JsonlBridge<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    fn request<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        if T::OPCODE != mutsuki_runtime_wire::Opcode::PluginInitialize {
            self.ensure_initialized()?;
        }
        self.request_without_handshake(request)
    }

    fn ensure_initialized(&self) -> RuntimeResult<()> {
        let mut handshake = self
            .handshake
            .lock()
            .map_err(|_| transport_failure("handshake lock poisoned"))?;
        if let Some(result) = handshake.as_ref() {
            return result.clone().map_err(RuntimeFailure::new);
        }
        let limits = self.current_limits()?;
        let hello = ProtocolHello::debug_jsonl_with_limits(limits)
            .map_err(|error| transport_failure(&error.to_string()))?;
        let result = self
            .request_without_handshake(&InitializeRequest {
                hello: hello.clone(),
                config: None,
            })
            .and_then(|ack| {
                ack.validate_for(&hello)
                    .map_err(|error| transport_failure(&error.to_string()))?;
                let negotiated = WireLimits {
                    max_frame_bytes: ack.max_frame_bytes as usize,
                    max_payload_bytes: ack.max_payload_bytes as usize,
                    max_in_flight_requests: ack.max_in_flight_requests as usize,
                    management_reserved_requests: ack.management_reserved_requests as usize,
                    ..limits
                };
                self.set_limits(negotiated)
            });
        if let Err(error) = &result {
            self.multiplexer.fail(error.error().clone());
        }
        *handshake = Some(match &result {
            Ok(()) => Ok(()),
            Err(error) => Err(error.error().clone()),
        });
        result
    }

    fn request_without_handshake<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        let request_id = self.multiplexer.next_request_id()?;
        let limits = self.current_limits()?;
        let encoded = encode_jsonl_request(request_id, request, limits)
            .map_err(|error| transport_failure(&error.to_string()))?;
        let response = self
            .multiplexer
            .request(request_id, T::OPCODE.is_management(), encoded)?;
        match decode_jsonl_response::<T>(&response, request_id, limits) {
            Ok(response) => Ok(response),
            Err(error) => {
                self.multiplexer.fail(error.clone());
                Err(RuntimeFailure::new(error))
            }
        }
    }

    fn current_limits(&self) -> RuntimeResult<WireLimits> {
        self.limits
            .lock()
            .map(|limits| *limits)
            .map_err(|_| transport_failure("wire limits lock poisoned"))
    }

    fn set_limits(&self, limits: WireLimits) -> RuntimeResult<()> {
        self.multiplexer.set_limits(limits)?;
        *self
            .limits
            .lock()
            .map_err(|_| transport_failure("wire limits lock poisoned"))? = limits;
        Ok(())
    }
}

impl<R, W> JsonlRunner<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&ExportPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    pub fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&CommandPlanRequest {
            provider_id: None,
            plan: plan.clone(),
        })
    }

    pub fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport.request(&CommandBatchRequest {
            provider_id: None,
            batch: batch.clone(),
        })
    }

    pub fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport.request(&SagaPlanRequest {
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
        self.transport.request(&RunBatchRequest {
            runner_id: self.descriptor.runner_id.clone(),
            ctx,
            batch,
        })
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.transport.request(&CancelRunnerRequest {
            runner_id: self.descriptor.runner_id.clone(),
            invocation_id: invocation_id.into(),
        })
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.transport.request(&DisposeRunnerRequest {
            runner_id: self.descriptor.runner_id.clone(),
        })
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        Some(Arc::new(JsonlManagement {
            runner_id: self.descriptor.runner_id.clone(),
            transport: self.transport.clone(),
        }))
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
