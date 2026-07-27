//! Domain-neutral ABI v2 dynamic-library loading and connection assembly.

use std::ffi::c_void;
use std::io::{BufRead, Cursor, Read, Write};
use std::path::PathBuf;
use std::ptr;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use libloading::Library;
use mutsuki_runtime_contracts::{
    ArtifactType, PluginDeploymentKind, PluginManifest, RuntimeError, ScalarValue,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::abi::{
    ABI_V2_BRIDGE_ID, ABI_V2_CODEC_ID, ABI_V2_ENTRY_SYMBOL, ABI_V2_TRANSPORT_VERSION, AbiBuffer,
    AbiCallResult, AbiEntryV2, AbiHostV2, AbiPluginV2,
};
use mutsuki_runtime_sdk::{
    LoadedPlugin, ResourcePlanGateway, RuntimeBootstrapperResourceProvider, TaskSubmitter,
    dispatch_binary_host_request,
};
use mutsuki_runtime_wire::{
    DEFAULT_WIRE_LIMITS, ProtocolHello, WireFlags, WireRequest, decode_binary_frame,
};
use serde_json::Value;

use crate::{BinaryTransport, TransportResourceProvider, TransportRunner, TypedRequestTransport};

const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const WORK_QUEUE_LIMIT: usize = 56;
const MANAGEMENT_QUEUE_LIMIT: usize = 8;

/// Inputs required to connect an already discovered and validated ABI v2 artifact.
///
/// Package discovery, archive extraction, path/hash validation, cache selection and reload
/// generation ownership stay with the product Host.
pub struct AbiPluginLoadRequest {
    pub library_path: PathBuf,
    pub expected_manifest: PluginManifest,
    pub config: Option<Value>,
    pub task_submitter: Arc<dyn TaskSubmitter>,
    pub resource_gateway: Arc<dyn ResourcePlanGateway>,
}

/// Loads, initializes and validates one ABI v2 plugin as a normal [`LoadedPlugin`].
pub fn load_abi_plugin_v2(request: AbiPluginLoadRequest) -> RuntimeResult<LoadedPlugin> {
    validate_expected_manifest(&request.expected_manifest)?;
    let plugin_id = request.expected_manifest.plugin_id.clone();
    let connection = Arc::new(AbiPluginConnection::open(
        &plugin_id,
        request.library_path,
        request.task_submitter,
        request.resource_gateway,
    )?);
    let hello = ProtocolHello::binary();
    let ack = connection
        .initialize(request.config)
        .map_err(|error| with_abi_context(error, &plugin_id, "abi.v2.initialize"))?;
    ack.validate_for(&hello)
        .map_err(|error| abi_failure(&plugin_id, "abi.v2.handshake", error.to_string()))?;
    let initialized = ack.plugin.ok_or_else(|| {
        abi_failure(
            &plugin_id,
            "abi.v2.surface_missing",
            "Wire handshake omitted initialized plugin surface",
        )
    })?;
    validate_initialized_plugin(&request.expected_manifest, &initialized)?;

    let runners = request
        .expected_manifest
        .provides
        .runners
        .iter()
        .cloned()
        .map(|descriptor| {
            Box::new(TransportRunner::new(descriptor, connection.clone())) as Box<dyn Runner>
        })
        .collect();
    let resource_providers = initialized
        .resource_provider_ids
        .into_iter()
        .map(|provider_id| RuntimeBootstrapperResourceProvider {
            provider_id: provider_id.clone(),
            provider: Arc::new(TransportResourceProvider::new(
                provider_id,
                connection.clone(),
            )),
        })
        .collect();

    Ok(LoadedPlugin {
        manifest: request.expected_manifest,
        runners,
        async_handlers: Vec::new(),
        host_services: Vec::new(),
        resource_providers,
        async_resource_providers: Vec::new(),
    })
}

fn validate_expected_manifest(manifest: &PluginManifest) -> RuntimeResult<()> {
    if manifest.artifact.artifact_type != ArtifactType::Abi {
        return Err(abi_failure(
            &manifest.plugin_id,
            "abi.v2.artifact_type",
            "ABI v2 loading requires artifact_type = abi",
        ));
    }
    let backends = manifest
        .provides
        .plugin_backends
        .iter()
        .filter(|backend| backend.deployment_kind == PluginDeploymentKind::Abi)
        .collect::<Vec<_>>();
    let [backend] = backends.as_slice() else {
        return Err(abi_failure(
            &manifest.plugin_id,
            "abi.v2.backend",
            "manifest must declare exactly one ABI plugin backend",
        ));
    };
    if backend.codec_id.as_deref() != Some(ABI_V2_CODEC_ID)
        || backend.bridge_id.as_deref() != Some(ABI_V2_BRIDGE_ID)
    {
        return Err(abi_failure(
            &manifest.plugin_id,
            "abi.v2.backend",
            format!("ABI backend must use codec {ABI_V2_CODEC_ID} and bridge {ABI_V2_BRIDGE_ID}"),
        ));
    }
    Ok(())
}

fn validate_initialized_plugin(
    expected: &PluginManifest,
    initialized: &mutsuki_runtime_wire::InitializedPlugin,
) -> RuntimeResult<()> {
    let mut guest_manifest = initialized.manifest.clone();
    // The product Host owns the installed path, hashes and companion staging identity.
    guest_manifest.artifact = expected.artifact.clone();
    if guest_manifest != *expected {
        return Err(abi_failure(
            &expected.plugin_id,
            "abi.v2.manifest_mismatch",
            "installed manifest differs from guest handshake",
        ));
    }

    let mut expected_providers = expected.provides.resource_providers.clone();
    let mut actual_providers = initialized.resource_provider_ids.clone();
    expected_providers.sort();
    actual_providers.sort();
    if expected_providers != actual_providers {
        return Err(abi_failure(
            &expected.plugin_id,
            "abi.v2.provider_surface_mismatch",
            "resource provider ids differ from guest handshake",
        ));
    }
    Ok(())
}

struct HostCallbackContext {
    task_submitter: Arc<dyn TaskSubmitter>,
    resource_gateway: Arc<dyn ResourcePlanGateway>,
}

unsafe extern "C" fn host_request(
    context: *mut c_void,
    request: *const u8,
    request_len: usize,
) -> AbiCallResult {
    if context.is_null() || (request.is_null() && request_len != 0) {
        return AbiCallResult::failed(b"invalid host callback pointers".to_vec());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `PluginLifetime` owns this context until every transport handle is dropped.
        let context = unsafe { &*context.cast::<HostCallbackContext>() };
        let request = if request_len == 0 {
            &[]
        } else {
            // SAFETY: the ABI callback contract keeps this buffer valid for this call.
            unsafe { std::slice::from_raw_parts(request, request_len) }
        };
        dispatch_binary_host_request(
            context.task_submitter.as_ref(),
            context.resource_gateway.as_ref(),
            request,
        )
    }));
    match result {
        Ok(response) => AbiCallResult::ok(response),
        Err(_) => AbiCallResult::failed(b"host ABI v2 callback panicked".to_vec()),
    }
}

unsafe extern "C" fn host_release(buffer: AbiBuffer) {
    if buffer.ptr.is_null() || buffer.len == 0 {
        return;
    }
    let slice = ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len);
    // SAFETY: host callback responses are allocated by `AbiBuffer::from_bytes`.
    unsafe { drop(Box::from_raw(slice)) };
}

struct PluginLifetime {
    api: AbiPluginV2,
    _library: Library,
    _host_context: Box<HostCallbackContext>,
}

// SAFETY: the ABI v2 contract requires plugin callbacks and context to be thread-safe for the
// bounded transport workers. The loaded library remains alive for the full callback lifetime.
unsafe impl Send for PluginLifetime {}
unsafe impl Sync for PluginLifetime {}

impl Drop for PluginLifetime {
    fn drop(&mut self) {
        if let Some(close) = self.api.close {
            // SAFETY: a validated ABI connection owns this context and closes it exactly once.
            unsafe { close(self.api.context) };
        }
    }
}

struct AbiPluginConnection {
    transport: BinaryTransport<CallbackReader, CallbackWriter>,
    _lifetime: Arc<PluginLifetime>,
}

impl AbiPluginConnection {
    fn open(
        plugin_id: &str,
        library_path: PathBuf,
        task_submitter: Arc<dyn TaskSubmitter>,
        resource_gateway: Arc<dyn ResourcePlanGateway>,
    ) -> RuntimeResult<Self> {
        let host_context = Box::new(HostCallbackContext {
            task_submitter,
            resource_gateway,
        });
        let host = AbiHostV2 {
            context: (&*host_context as *const HostCallbackContext)
                .cast_mut()
                .cast(),
            request: Some(host_request),
            release: Some(host_release),
        };
        // SAFETY: the caller supplied an already validated native library path. `Library` is kept
        // alive by `PluginLifetime` until all plugin callbacks have stopped.
        let library = unsafe { Library::new(&library_path) }.map_err(|error| {
            abi_failure(
                plugin_id,
                "abi.v2.library_open",
                format!("load {}: {error}", library_path.display()),
            )
        })?;
        // SAFETY: the symbol name and function signature are the published ABI v2 contract.
        let entry: AbiEntryV2 = unsafe {
            *library
                .get::<AbiEntryV2>(ABI_V2_ENTRY_SYMBOL)
                .map_err(|error| {
                    abi_failure(
                        plugin_id,
                        "abi.v2.symbol_missing",
                        format!(
                            "missing {}: {error}",
                            String::from_utf8_lossy(ABI_V2_ENTRY_SYMBOL)
                        ),
                    )
                })?
        };
        // SAFETY: the loaded symbol has the validated ABI v2 entry signature.
        let api = unsafe { entry(host) };
        if api.transport_version != ABI_V2_TRANSPORT_VERSION
            || api.context.is_null()
            || api.request.is_none()
            || api.release.is_none()
            || api.close.is_none()
        {
            if !api.context.is_null()
                && let Some(close) = api.close
            {
                // SAFETY: a partially valid entry still transfers ownership of a non-null context.
                unsafe { close(api.context) };
            }
            return Err(abi_failure(
                plugin_id,
                "abi.v2.entry_invalid",
                format!(
                    "invalid ABI v2 entry or transport version {}, expected {}",
                    api.transport_version, ABI_V2_TRANSPORT_VERSION
                ),
            ));
        }

        let lifetime = Arc::new(PluginLifetime {
            api,
            _library: library,
            _host_context: host_context,
        });
        let (reader, writer) = callback_io(lifetime.clone());
        let transport = BinaryTransport::with_limits(
            reader,
            writer,
            DEFAULT_WIRE_LIMITS,
            DEFAULT_RESPONSE_TIMEOUT,
        )
        .map_err(|error| with_abi_context(error, plugin_id, "abi.v2.transport"))?;
        Ok(Self {
            transport,
            _lifetime: lifetime,
        })
    }

    fn initialize(
        &self,
        config: Option<Value>,
    ) -> RuntimeResult<mutsuki_runtime_wire::ProtocolHelloAck> {
        self.transport.initialize(config)
    }
}

impl TypedRequestTransport for AbiPluginConnection {
    fn request<R: WireRequest>(&self, request: &R) -> RuntimeResult<R::Response> {
        self.transport.request(request)
    }
}

struct CallbackReader {
    receiver: Receiver<Vec<u8>>,
    current: Cursor<Vec<u8>>,
}

struct CallbackWriter {
    work: SyncSender<Vec<u8>>,
    management: SyncSender<Vec<u8>>,
    buffer: Vec<u8>,
}

fn callback_io(lifetime: Arc<PluginLifetime>) -> (CallbackReader, CallbackWriter) {
    let (work_tx, work_rx) = mpsc::sync_channel(WORK_QUEUE_LIMIT);
    let (management_tx, management_rx) = mpsc::sync_channel(MANAGEMENT_QUEUE_LIMIT);
    let (response_tx, response_rx) = mpsc::channel();
    spawn_callback_workers(4, work_rx, response_tx.clone(), lifetime.clone());
    spawn_callback_workers(1, management_rx, response_tx, lifetime);
    (
        CallbackReader {
            receiver: response_rx,
            current: Cursor::new(Vec::new()),
        },
        CallbackWriter {
            work: work_tx,
            management: management_tx,
            buffer: Vec::new(),
        },
    )
}

fn spawn_callback_workers(
    count: usize,
    receiver: Receiver<Vec<u8>>,
    response: mpsc::Sender<Vec<u8>>,
    lifetime: Arc<PluginLifetime>,
) {
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..count {
        let receiver = receiver.clone();
        let response = response.clone();
        let lifetime = lifetime.clone();
        std::thread::Builder::new()
            .name(format!("mutsuki-abi-v2-callback-{index}"))
            .spawn(move || {
                loop {
                    let frame = {
                        let receiver = receiver.lock().expect("ABI v2 queue poisoned");
                        receiver.recv()
                    };
                    let Ok(frame) = frame else { break };
                    if response.send(invoke_plugin(&lifetime, &frame)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn ABI v2 callback worker");
    }
}

fn invoke_plugin(lifetime: &PluginLifetime, frame: &[u8]) -> Vec<u8> {
    let api = lifetime.api;
    let callback = api.request.expect("validated ABI v2 request callback");
    // SAFETY: `PluginLifetime` owns a validated plugin context for the duration of this call.
    let result = unsafe { callback(api.context, frame.as_ptr(), frame.len()) };
    let valid = (result.payload.len == 0 && result.payload.ptr.is_null())
        || (result.payload.len > 0 && !result.payload.ptr.is_null());
    if !valid {
        return Vec::new();
    }
    // SAFETY: the validated pointer/length pair is borrowed until the paired release call.
    let response = unsafe { result.payload.as_slice() }.to_vec();
    if result.payload.len > 0 {
        // SAFETY: non-empty plugin buffers have a validated release callback.
        unsafe { api.release.expect("validated ABI v2 release")(result.payload) };
    }
    if result.status == 0 {
        response
    } else {
        Vec::new()
    }
}

impl CallbackReader {
    fn refill(&mut self) -> std::io::Result<()> {
        if self.current.position() as usize >= self.current.get_ref().len() {
            let frame = self.receiver.recv().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "ABI v2 response closed")
            })?;
            self.current = Cursor::new(frame);
        }
        Ok(())
    }
}

impl Read for CallbackReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.refill()?;
        self.current.read(buffer)
    }
}

impl BufRead for CallbackReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.refill()?;
        self.current.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.current.consume(amount);
    }
}

impl Write for CallbackWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let frame = std::mem::take(&mut self.buffer);
        let decoded = decode_binary_frame(&frame, DEFAULT_WIRE_LIMITS)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let sender = if decoded.header.flags.contains(WireFlags::MANAGEMENT) {
            &self.management
        } else {
            &self.work
        };
        sender
            .send(frame)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "ABI v2 queue closed"))
    }
}

fn abi_failure(
    plugin_id: &str,
    route: impl Into<String>,
    detail: impl Into<String>,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        format!("plugin:{plugin_id}"),
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

fn with_abi_context(
    failure: RuntimeFailure,
    plugin_id: &str,
    route: &'static str,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        format!("plugin:{plugin_id}"),
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(failure.to_string()));
    error.evidence.insert(
        "cause_code".into(),
        ScalarValue::String(failure.error().code.clone()),
    );
    RuntimeFailure::new(error)
}
