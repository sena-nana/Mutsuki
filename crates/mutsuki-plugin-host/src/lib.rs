//! Core-independent ABI v2 plugin loader and lifecycle host.
//!
//! Product hosts own package discovery, staging, configuration selection and persistence. This
//! crate only owns one already validated dynamic library connection and exposes its runners.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use libloading::Library;
use mutsuki_plugin_api::{
    ABI_V2_BRIDGE_ID, ABI_V2_CODEC_ID, ABI_V2_ENTRY_SYMBOL, ABI_V2_TRANSPORT_VERSION, AbiBuffer,
    AbiCallResult, AbiEntryV2, AbiHostV2, AbiPluginV2, PluginHostContext, PluginHostError,
    PluginResult, consume_call_result, plugin_error,
};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, PluginDeploymentKind, PluginManifest, RunnerContext,
    RunnerDescriptor, WorkBatch,
};
use mutsuki_runtime_wire::{
    CancelRunnerRequest, DisposeRunnerRequest, InitializeRequest, ProtocolHello, RunBatchRequest,
    WireRequest, decode_binary_response, encode_binary_request,
};
use serde_json::Value;

const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_WORK_QUEUE_LIMIT: usize = 56;
const DEFAULT_MANAGEMENT_QUEUE_LIMIT: usize = 8;
const DEFAULT_WORKER_COUNT: usize = 4;
const RETIRED_ABI_V1_ENTRY_SYMBOL: &[u8] = b"mutsuki_plugin_abi_v1\0";

#[derive(Clone, Debug)]
pub struct PluginHostConfig {
    pub response_timeout: Duration,
    pub work_queue_limit: usize,
    pub management_queue_limit: usize,
    pub work_workers: usize,
}

impl Default for PluginHostConfig {
    fn default() -> Self {
        Self {
            response_timeout: DEFAULT_RESPONSE_TIMEOUT,
            work_queue_limit: DEFAULT_WORK_QUEUE_LIMIT,
            management_queue_limit: DEFAULT_MANAGEMENT_QUEUE_LIMIT,
            work_workers: DEFAULT_WORKER_COUNT,
        }
    }
}

pub struct PluginLoadRequest {
    pub library_path: PathBuf,
    pub expected_manifest: PluginManifest,
    pub config: Option<Value>,
    pub host_context: PluginHostContext,
    pub host_config: PluginHostConfig,
}

pub struct PluginSession {
    manifest: PluginManifest,
    initialized: mutsuki_runtime_wire::InitializedPlugin,
    connection: Arc<PluginConnection>,
    runners: BTreeMap<mutsuki_runtime_contracts::RunnerId, PluginRunnerHandle>,
    disposed: Mutex<bool>,
}

#[derive(Clone)]
pub struct PluginRunnerHandle {
    descriptor: RunnerDescriptor,
    connection: Arc<PluginConnection>,
}

impl PluginSession {
    pub fn load(request: PluginLoadRequest) -> PluginResult<Self> {
        validate_expected_manifest(&request.expected_manifest)?;
        let plugin_id = request.expected_manifest.plugin_id.clone();
        let connection = Arc::new(PluginConnection::open(
            plugin_id.as_str(),
            request.library_path,
            request.host_context,
            request.host_config,
        )?);
        let hello = ProtocolHello::binary();
        let ack = connection
            .request(&InitializeRequest {
                hello: hello.clone(),
                config: request.config,
            })
            .map_err(|error| with_context(error, &plugin_id, "abi.v2.initialize"))?;
        ack.validate_for(&hello)
            .map_err(|error| plugin_failure(&plugin_id, "abi.v2.handshake", error.to_string()))?;
        let initialized = ack.plugin.ok_or_else(|| {
            plugin_failure(
                &plugin_id,
                "abi.v2.surface_missing",
                "wire handshake omitted initialized plugin surface",
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
                let id = mutsuki_runtime_contracts::RunnerId::from(descriptor.runner_id.clone());
                (
                    id,
                    PluginRunnerHandle {
                        descriptor,
                        connection: connection.clone(),
                    },
                )
            })
            .collect();
        Ok(Self {
            manifest: request.expected_manifest,
            initialized,
            connection,
            runners,
            disposed: Mutex::new(false),
        })
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn initialized(&self) -> &mutsuki_runtime_wire::InitializedPlugin {
        &self.initialized
    }

    pub fn runner(&self, runner_id: &str) -> Option<PluginRunnerHandle> {
        self.runners.get(runner_id).cloned()
    }

    pub fn runners(&self) -> impl Iterator<Item = &PluginRunnerHandle> {
        self.runners.values()
    }

    pub fn dispose(&self) -> PluginResult<()> {
        let mut disposed = self.disposed.lock().expect("plugin dispose mutex");
        if *disposed {
            return Ok(());
        }
        for runner_id in self.runners.keys() {
            self.connection.request(&DisposeRunnerRequest {
                runner_id: runner_id.to_string(),
            })?;
        }
        *disposed = true;
        Ok(())
    }

    /// Sends a typed wire request through this session's bounded ABI transport.
    ///
    /// The generic request surface is intentionally kept at the plugin host boundary so
    /// compatibility adapters can reuse the same connection without depending on CoreRuntime.
    pub fn request<R: WireRequest>(&self, request: &R) -> PluginResult<R::Response> {
        if *self.disposed.lock().expect("plugin dispose mutex") {
            return Err(plugin_error(
                "plugin.session.disposed",
                "plugin session is disposed",
            ));
        }
        self.connection.request(request)
    }
}

impl Drop for PluginSession {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

impl PluginRunnerHandle {
    pub fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    pub fn run_batch(&self, ctx: RunnerContext, batch: WorkBatch) -> PluginResult<CompletionBatch> {
        let lease_ids = batch
            .task_leases
            .iter()
            .map(|lease| lease.lease_id.clone())
            .collect::<Vec<_>>();
        if lease_ids != ctx.task_lease_ids {
            return Err(plugin_failure(
                &self.descriptor.plugin_id,
                "abi.runner.claim_conflict",
                format!("runner.run_batch.{}", batch.batch_id),
            ));
        }
        self.connection.request(&RunBatchRequest {
            runner_id: self.descriptor.runner_id.to_string(),
            ctx,
            batch,
        })
    }

    pub fn cancel(&self, invocation_id: &str) -> PluginResult<()> {
        self.connection.request(&CancelRunnerRequest {
            runner_id: self.descriptor.runner_id.to_string(),
            invocation_id: invocation_id.into(),
        })
    }
}

fn validate_expected_manifest(manifest: &PluginManifest) -> PluginResult<()> {
    if manifest.artifact.artifact_type != ArtifactType::Abi {
        return Err(plugin_failure(
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
        return Err(plugin_failure(
            &manifest.plugin_id,
            "abi.v2.backend",
            "manifest must declare exactly one ABI plugin backend",
        ));
    };
    if backend.codec_id.as_deref() != Some(ABI_V2_CODEC_ID)
        || backend.bridge_id.as_deref() != Some(ABI_V2_BRIDGE_ID)
    {
        return Err(plugin_failure(
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
) -> PluginResult<()> {
    let mut guest_manifest = initialized.manifest.clone();
    guest_manifest.artifact = expected.artifact.clone();
    if guest_manifest != *expected {
        return Err(plugin_failure(
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
        return Err(plugin_failure(
            &expected.plugin_id,
            "abi.v2.provider_surface_mismatch",
            "resource provider ids differ from guest handshake",
        ));
    }
    Ok(())
}

struct HostCallbackContext {
    host: PluginHostContext,
}

unsafe extern "C" fn host_request(
    context: *mut std::ffi::c_void,
    request: *const u8,
    request_len: usize,
) -> AbiCallResult {
    if context.is_null() || (request.is_null() && request_len != 0) {
        return AbiCallResult::failed(b"invalid host callback pointers".to_vec());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &*context.cast::<HostCallbackContext>() };
        let request = if request_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(request, request_len) }
        };
        context.host.dispatch_binary_request(request)
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
    let slice = ptr_slice(buffer);
    unsafe {
        drop(Box::from_raw(slice));
    }
}

fn ptr_slice(buffer: AbiBuffer) -> *mut [u8] {
    std::ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len)
}

struct PluginLifetime {
    api: AbiPluginV2,
    _library: Library,
    _host_context: Box<HostCallbackContext>,
}

unsafe impl Send for PluginLifetime {}
unsafe impl Sync for PluginLifetime {}

impl Drop for PluginLifetime {
    fn drop(&mut self) {
        if let Some(close) = self.api.close {
            unsafe {
                close(self.api.context);
            }
        }
    }
}

struct PluginConnection {
    _lifetime: Arc<PluginLifetime>,
    work: SyncSender<Vec<u8>>,
    management: SyncSender<Vec<u8>>,
    response: Mutex<Receiver<Vec<u8>>>,
    request_lock: Mutex<()>,
    next_request_id: AtomicU64,
    response_timeout: Duration,
}

impl PluginConnection {
    fn open(
        plugin_id: &str,
        library_path: PathBuf,
        host_context: PluginHostContext,
        config: PluginHostConfig,
    ) -> PluginResult<Self> {
        let host_context = Box::new(HostCallbackContext { host: host_context });
        let host = AbiHostV2 {
            context: (&*host_context as *const HostCallbackContext)
                .cast_mut()
                .cast(),
            request: Some(host_request),
            release: Some(host_release),
        };
        let library = unsafe { Library::new(&library_path) }.map_err(|error| {
            plugin_failure(
                plugin_id,
                "abi.v2.library_open",
                format!("load {}: {error}", library_path.display()),
            )
        })?;
        let entry: AbiEntryV2 = unsafe {
            match library.get::<AbiEntryV2>(ABI_V2_ENTRY_SYMBOL) {
                Ok(entry) => *entry,
                Err(error) => {
                    if library
                        .get::<*const ()>(RETIRED_ABI_V1_ENTRY_SYMBOL)
                        .is_ok()
                    {
                        return Err(plugin_failure_code(
                            plugin_id,
                            "abi.unsupported_version",
                            "abi.v2.symbol_missing",
                            "ABI v1 is retired; rebuild the plugin for ABI v2",
                        ));
                    }
                    return Err(plugin_failure(
                        plugin_id,
                        "abi.v2.symbol_missing",
                        format!(
                            "missing {}: {error}",
                            String::from_utf8_lossy(ABI_V2_ENTRY_SYMBOL)
                        ),
                    ));
                }
            }
        };
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
                unsafe {
                    close(api.context);
                }
            }
            return Err(plugin_failure(
                plugin_id,
                "abi.v2.entry_invalid",
                format!(
                    "invalid ABI v2 entry or transport version {}",
                    api.transport_version
                ),
            ));
        }
        let lifetime = Arc::new(PluginLifetime {
            api,
            _library: library,
            _host_context: host_context,
        });
        let (work, work_rx) = mpsc::sync_channel(config.work_queue_limit.max(1));
        let (management, management_rx) = mpsc::sync_channel(config.management_queue_limit.max(1));
        let (response_tx, response_rx) = mpsc::channel();
        spawn_workers(
            config.work_workers.max(1),
            work_rx,
            response_tx.clone(),
            lifetime.clone(),
        );
        spawn_workers(1, management_rx, response_tx, lifetime.clone());
        Ok(Self {
            _lifetime: lifetime,
            work,
            management,
            response: Mutex::new(response_rx),
            request_lock: Mutex::new(()),
            next_request_id: AtomicU64::new(1),
            response_timeout: config.response_timeout,
        })
    }

    fn request<R: WireRequest>(&self, request: &R) -> PluginResult<R::Response> {
        let _request_guard = self
            .request_lock
            .lock()
            .map_err(|_| plugin_error("abi.v2.request_lock", "plugin request lock poisoned"))?;
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed).max(1);
        let frame = encode_binary_request(
            request_id,
            request,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
        .map_err(|error| plugin_failure("plugin", "abi.v2.encode", error.to_string()))?;
        let is_management = R::OPCODE.is_management();
        if is_management {
            self.management
                .send(frame)
                .map_err(|_| plugin_error("abi.v2.management_queue", "management queue closed"))?;
        } else {
            self.work
                .send(frame)
                .map_err(|_| plugin_error("abi.v2.work_queue", "work queue closed"))?;
        }
        let response = self
            .response
            .lock()
            .map_err(|_| plugin_error("abi.v2.response_lock", "response queue poisoned"))?
            .recv_timeout(self.response_timeout)
            .map_err(|error| plugin_failure("plugin", "abi.v2.response", error.to_string()))?;
        decode_binary_response::<R>(
            &response,
            request_id,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
        .map_err(|error| plugin_error("abi.v2.decode_response", format!("{error:?}")))
    }
}

fn spawn_workers(
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
            .name(format!("mutsuki-plugin-host-worker-{index}"))
            .spawn(move || {
                loop {
                    let frame = receiver.lock().ok().and_then(|queue| queue.recv().ok());
                    let Some(frame) = frame else { break };
                    let response_frame = invoke_plugin(&lifetime, &frame);
                    if response.send(response_frame).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn plugin host worker");
    }
}

fn invoke_plugin(lifetime: &PluginLifetime, frame: &[u8]) -> Vec<u8> {
    let callback = lifetime
        .api
        .request
        .expect("validated plugin request callback");
    let result = unsafe { callback(lifetime.api.context, frame.as_ptr(), frame.len()) };
    let Ok((ok, bytes)) =
        consume_call_result(result, lifetime.api.release, "abi.v2.plugin_callback")
    else {
        return Vec::new();
    };
    if ok { bytes } else { Vec::new() }
}

fn plugin_failure(
    plugin_id: impl AsRef<str>,
    route: &str,
    detail: impl Into<String>,
) -> PluginHostError {
    let mut error = plugin_error(route, detail);
    error.error.source = format!("plugin:{}", plugin_id.as_ref());
    error
}

fn plugin_failure_code(
    plugin_id: impl AsRef<str>,
    code: &str,
    route: &str,
    detail: impl Into<String>,
) -> PluginHostError {
    PluginHostError::new(
        code,
        format!("plugin:{}", plugin_id.as_ref()),
        route,
        detail,
    )
}

fn with_context(
    error: PluginHostError,
    plugin_id: impl AsRef<str>,
    route: &str,
) -> PluginHostError {
    plugin_failure(plugin_id, route, format!("{error}"))
}
