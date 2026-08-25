//! Core-independent ABI v2 plugin loader and lifecycle host.
//!
//! Product hosts own package discovery, staging, configuration selection and persistence. This
//! crate only owns one already validated dynamic library connection and exposes its runners.
//!
//! # Unsafe boundary
//!
//! This is one of the few crates on the workspace `unsafe_code` exception list. Loading a
//! dynamic library and calling across the ABI v2 `extern "C"` surface cannot be expressed in
//! safe Rust. Every `unsafe` block carries its own `SAFETY:` argument, and the library handle
//! is kept alive by `PluginLifetime` for as long as any worker can still call into it.
#![allow(unsafe_code)]
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::borrow_as_ptr,
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::useless_conversion
)]

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
        // SAFETY: `context` was checked non-null above and is the pointer this crate handed
        // to the plugin in `PluginConnection::open`. The boxed `HostCallbackContext` is owned
        // by `PluginLifetime`, which outlives every plugin call that can reach this callback.
        let context = unsafe { &*context.cast::<HostCallbackContext>() };
        let request = if request_len == 0 {
            &[]
        } else {
            // SAFETY: the null/length combinations rejected above leave only a non-null
            // pointer to `request_len` initialised bytes owned by the calling plugin. The
            // borrow ends before this callback returns.
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
    // SAFETY: non-empty buffers reaching this callback were produced by this process through
    // `AbiBuffer::from_bytes`, which leaks a `Box<[u8]>` of exactly `len` elements. The ABI
    // contract makes release single-shot, so this is the matching free.
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

// SAFETY: the raw pointers inside `AbiPluginV2` are an opaque plugin context plus `extern "C"`
// function pointers. ABI v2 requires a plugin to accept those calls from any thread and to
// synchronise its own context, which is what lets the worker pool share one lifetime. The
// `Library` handle is owned here and is only dropped after `close`, so no worker can observe
// an unloaded image.
unsafe impl Send for PluginLifetime {}
unsafe impl Sync for PluginLifetime {}

impl Drop for PluginLifetime {
    fn drop(&mut self) {
        if let Some(close) = self.api.close {
            // SAFETY: `PluginLifetime` is held behind an `Arc` by every worker, so reaching
            // `drop` proves no further request can be in flight. `close` is called exactly
            // once here, before `_library` unloads the image.
            unsafe {
                close(self.api.context);
            }
        }
    }
}

/// One in-flight ABI request, carrying its own reply channel.
///
/// Routing the response through a per-call channel rather than one shared response queue is what
/// lets several requests be in flight at once: a shared queue cannot tell which waiter a frame
/// belongs to, so it forces every call to hold a global lock and serialises the whole worker pool
/// down to one request at a time.
struct PluginCall {
    request_id: u64,
    opcode: mutsuki_runtime_wire::Opcode,
    frame: Vec<u8>,
    reply: SyncSender<Vec<u8>>,
}

struct PluginConnection {
    _lifetime: Arc<PluginLifetime>,
    work: SyncSender<PluginCall>,
    management: SyncSender<PluginCall>,
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
            context: std::ptr::from_ref::<HostCallbackContext>(&*host_context)
                .cast_mut()
                .cast(),
            request: Some(host_request),
            release: Some(host_release),
        };
        // SAFETY: loading an arbitrary dynamic library runs its initialisers, so soundness
        // rests on the caller having validated provenance. Product hosts resolve the path
        // inside the plugin root and verify the artifact hash before reaching this point.
        let library = unsafe { Library::new(&library_path) }.map_err(|error| {
            plugin_failure(
                plugin_id,
                "abi.v2.library_open",
                format!("load {}: {error}", library_path.display()),
            )
        })?;
        // SAFETY: resolving a symbol asserts that the exported signature matches `AbiEntryV2`.
        // That is the ABI v2 contract the artifact declares in its manifest; a mismatch is a
        // packaging error the loader cannot detect, which is why the entry is validated
        // immediately after the call and the connection is rejected on any inconsistency.
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
        // SAFETY: `entry` was resolved from the ABI v2 entry symbol and `host` is a fully
        // initialised `AbiHostV2` whose context outlives the connection.
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
                // SAFETY: the entry returned a context and a close callback, and no request
                // has been issued yet, so this is the single teardown for a rejected handshake.
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
        spawn_workers(config.work_workers.max(1), work_rx, lifetime.clone());
        // Management stays single-threaded: initialize, cancel and dispose are ordered against
        // each other, and interleaving them would let a dispose overtake the call it terminates.
        spawn_workers(1, management_rx, lifetime.clone());
        Ok(Self {
            _lifetime: lifetime,
            work,
            management,
            next_request_id: AtomicU64::new(1),
            response_timeout: config.response_timeout,
        })
    }

    fn request<R: WireRequest>(&self, request: &R) -> PluginResult<R::Response> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed).max(1);
        let frame = encode_binary_request(
            request_id,
            request,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
        .map_err(|error| plugin_failure("plugin", "abi.v2.encode", error.to_string()))?;
        let (reply, response_rx) = mpsc::sync_channel(1);
        let call = PluginCall {
            request_id,
            opcode: R::OPCODE,
            frame,
            reply,
        };
        if R::OPCODE.is_management() {
            self.management
                .send(call)
                .map_err(|_| plugin_error("abi.v2.management_queue", "management queue closed"))?;
        } else {
            self.work
                .send(call)
                .map_err(|_| plugin_error("abi.v2.work_queue", "work queue closed"))?;
        }
        let response = response_rx
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

fn spawn_workers(count: usize, receiver: Receiver<PluginCall>, lifetime: Arc<PluginLifetime>) {
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..count {
        let receiver = receiver.clone();
        let lifetime = lifetime.clone();
        std::thread::Builder::new()
            .name(format!("mutsuki-plugin-host-worker-{index}"))
            .spawn(move || {
                loop {
                    let call = receiver.lock().ok().and_then(|queue| queue.recv().ok());
                    let Some(call) = call else { break };
                    let response_frame = invoke_plugin(&lifetime, &call);
                    // A closed reply channel means the caller already timed out. That is not a
                    // reason to stop serving the queue.
                    let _ = call.reply.send(response_frame);
                }
            })
            .expect("spawn plugin host worker");
    }
}

fn invoke_plugin(lifetime: &PluginLifetime, call: &PluginCall) -> Vec<u8> {
    let callback = lifetime
        .api
        .request
        .expect("validated plugin request callback");
    // The guest is expected to trap its own panics before they cross `extern "C"`, but the
    // result handling on this side can still fail. Catching here keeps one bad call from
    // killing a shared worker thread and silently shrinking the pool for every later request.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: the callback and context come from a handshake that validated both, and the
        // caller holds an `Arc<PluginLifetime>` so the library image is still loaded. The frame
        // is a live borrow for the duration of the call.
        let result =
            unsafe { callback(lifetime.api.context, call.frame.as_ptr(), call.frame.len()) };
        consume_call_result(result, lifetime.api.release, "abi.v2.plugin_callback")
    }));
    match outcome {
        Ok(Ok((true, bytes))) => bytes,
        Ok(Ok((false, bytes))) => abi_error_frame(
            call,
            "abi.v2.plugin_callback",
            String::from_utf8_lossy(&bytes).into_owned(),
        ),
        Ok(Err(error)) => abi_error_frame(call, "abi.v2.plugin_callback", error.to_string()),
        Err(_) => abi_error_frame(
            call,
            "abi.v2.plugin_callback_panicked",
            "plugin host worker panicked while handling the ABI response".into(),
        ),
    }
}

/// Builds a wire error response the caller can decode.
///
/// Returning an empty frame instead would surface as an opaque decode failure that names neither
/// the request nor the reason, which is the difference between a diagnosable plugin fault and a
/// mystery timeout.
fn abi_error_frame(call: &PluginCall, route: &str, detail: String) -> Vec<u8> {
    let mut error = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "plugin_host.abi_v2",
        route,
    );
    error.evidence.insert(
        "reason".into(),
        mutsuki_runtime_contracts::ScalarValue::String(detail),
    );
    mutsuki_runtime_wire::encode_binary_response::<()>(
        call.request_id,
        call.opcode,
        Err(&error),
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
    )
    .unwrap_or_default()
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
