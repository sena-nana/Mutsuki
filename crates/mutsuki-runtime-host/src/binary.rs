mod frame;

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_runtime_contracts::{CompletionBatch, RunnerDescriptor, RuntimeError, WorkBatch};
use mutsuki_runtime_core::{
    Runner, RunnerContext, RunnerManagementHandle, RuntimeFailure, RuntimeResult,
};
use mutsuki_runtime_wire::{
    CancelRunnerRequest, DEFAULT_WIRE_LIMITS, DisposeRunnerRequest, InitializeRequest,
    ProtocolHello, ProtocolHelloAck, RunBatchRequest, WireLimits, WireRequest,
    decode_binary_response, encode_binary_request,
};
use serde_json::Value;

use self::frame::BinaryFrameCodec;
use crate::multiplexer::{RequestMultiplexer, transport_failure};

const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct BinaryTransport<R, W> {
    bridge: Arc<BinaryBridge<R, W>>,
}

struct BinaryBridge<R, W> {
    multiplexer: RequestMultiplexer<R, W>,
    handshake: Mutex<Option<Result<ProtocolHelloAck, RuntimeError>>>,
    limits: Mutex<WireLimits>,
}

impl<R, W> Clone for BinaryTransport<R, W> {
    fn clone(&self) -> Self {
        Self {
            bridge: self.bridge.clone(),
        }
    }
}

impl<R, W> BinaryTransport<R, W>
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
            bridge: Arc::new(BinaryBridge {
                multiplexer: RequestMultiplexer::new(
                    reader,
                    writer,
                    BinaryFrameCodec::new(limits),
                    limits,
                    response_timeout,
                ),
                handshake: Mutex::new(None),
                limits: Mutex::new(limits),
            }),
        })
    }

    pub fn request<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        self.bridge.request(request)
    }

    pub fn initialize(&self, config: Option<Value>) -> RuntimeResult<ProtocolHelloAck> {
        self.bridge.initialize(config)
    }

    pub fn into_inner(self) -> (R, W) {
        match Arc::try_unwrap(self.bridge) {
            Ok(bridge) => bridge.multiplexer.into_inner(),
            Err(_) => panic!("binary transport still has active handles"),
        }
    }
}

impl<R, W> crate::TypedRequestTransport for BinaryTransport<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    fn request<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        BinaryTransport::request(self, request)
    }
}

impl<R, W> BinaryBridge<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    fn request<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        if T::OPCODE == mutsuki_runtime_wire::Opcode::PluginInitialize {
            return Err(transport_failure(
                "use BinaryTransport::initialize to establish handshake state",
            ));
        }
        self.ensure_initialized()?;
        self.request_without_handshake(request)
    }

    fn ensure_initialized(&self) -> RuntimeResult<()> {
        let mut handshake = self
            .handshake
            .lock()
            .map_err(|_| transport_failure("handshake lock poisoned"))?;
        if let Some(result) = handshake.as_ref() {
            return result
                .as_ref()
                .map(|_| ())
                .map_err(|error| RuntimeFailure::new(error.clone()));
        }
        self.initialize_locked(&mut handshake, None).map(|_| ())
    }

    fn initialize(&self, config: Option<Value>) -> RuntimeResult<ProtocolHelloAck> {
        let mut handshake = self
            .handshake
            .lock()
            .map_err(|_| transport_failure("handshake lock poisoned"))?;
        if handshake.is_some() {
            return Err(transport_failure(
                "binary transport handshake is already initialized",
            ));
        }
        self.initialize_locked(&mut handshake, config)
    }

    fn initialize_locked(
        &self,
        handshake: &mut Option<Result<ProtocolHelloAck, RuntimeError>>,
        config: Option<Value>,
    ) -> RuntimeResult<ProtocolHelloAck> {
        let limits = self.current_limits()?;
        let hello = ProtocolHello::binary_with_limits(limits)
            .map_err(|error| transport_failure(&error.to_string()))?;
        let result = self
            .request_without_handshake(&InitializeRequest {
                hello: hello.clone(),
                config,
            })
            .and_then(|ack| {
                ack.validate_for(&hello)
                    .map_err(|error| transport_failure(&error.to_string()))?;
                self.set_limits(WireLimits {
                    max_frame_bytes: ack.max_frame_bytes as usize,
                    max_payload_bytes: ack.max_payload_bytes as usize,
                    max_in_flight_requests: ack.max_in_flight_requests as usize,
                    management_reserved_requests: ack.management_reserved_requests as usize,
                    ..limits
                })?;
                Ok(ack)
            });
        if let Err(error) = &result {
            self.multiplexer.fail(error.error().clone());
        }
        *handshake = Some(
            result
                .as_ref()
                .cloned()
                .map_err(|error| error.error().clone()),
        );
        result
    }

    fn request_without_handshake<T: WireRequest>(&self, request: &T) -> RuntimeResult<T::Response> {
        let request_id = self.multiplexer.next_request_id()?;
        let limits = self.current_limits()?;
        let encoded = encode_binary_request(request_id, request, limits)
            .map_err(|error| transport_failure(&error.to_string()))?;
        let response = self
            .multiplexer
            .request(request_id, T::OPCODE.is_management(), encoded)?;
        match decode_binary_response::<T>(&response, request_id, limits) {
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

pub struct BinaryRunner<R, W> {
    descriptor: RunnerDescriptor,
    transport: BinaryTransport<R, W>,
}

impl<R, W> BinaryRunner<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn new(descriptor: RunnerDescriptor, reader: R, writer: W) -> Self {
        Self {
            descriptor,
            transport: BinaryTransport::new(reader, writer),
        }
    }

    pub fn transport(&self) -> BinaryTransport<R, W> {
        self.transport.clone()
    }
}

impl<R, W> Runner for BinaryRunner<R, W>
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
        Some(Arc::new(BinaryManagement {
            runner_id: self.descriptor.runner_id.clone(),
            transport: self.transport.clone(),
        }))
    }
}

struct BinaryManagement<R, W> {
    runner_id: String,
    transport: BinaryTransport<R, W>,
}

impl<R, W> std::fmt::Debug for BinaryManagement<R, W> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BinaryManagement")
    }
}

impl<R, W> RunnerManagementHandle for BinaryManagement<R, W>
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
