//! Core-independent ABI and host callback contracts for Mutsuki plugins.
//!
//! This crate deliberately contains only stable descriptors, FFI-safe ABI types and optional
//! host gateways. It does not own a task pool, executor, runtime actor or product host.
//!
//! # Unsafe boundary
//!
//! This is one of the few crates on the workspace `unsafe_code` exception list. It defines the
//! `extern "C"` guest side of ABI v2, so raw pointers, manual `Send`/`Sync` and `Box::from_raw`
//! are unavoidable here. Every `unsafe` block carries its own `SAFETY:` argument, and no other
//! module in this crate is allowed to grow one without the same justification.
#![allow(unsafe_code)]
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::manual_let_else,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::too_many_lines
)]

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RuntimeError, SnapshotDescriptor,
    StreamPlan, TaskBatch, TaskHandle, TaskOutcome,
};
use mutsuki_runtime_wire::{
    AnyWireRequest, DecodedWireRequest, Opcode, decode_binary_any_request, encode_binary_response,
};

pub const ABI_V2_TRANSPORT_VERSION: u32 = 2;
pub const ABI_V2_ENTRY_SYMBOL: &[u8] = b"mutsuki_plugin_abi_v2\0";
pub const ABI_V2_CODEC_ID: &str = mutsuki_runtime_wire::BINARY_CODEC_ID;
pub const ABI_V2_BRIDGE_ID: &str = "mutsuki.bridge.abi.binary.v2";

/// Structured error returned by the independent plugin API.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginHostError {
    pub error: RuntimeError,
}

impl PluginHostError {
    pub fn new(
        code: impl Into<String>,
        source: impl Into<String>,
        route: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        let mut error = RuntimeError::new(code, source, route);
        error.evidence.insert(
            "detail".into(),
            mutsuki_runtime_contracts::ScalarValue::String(detail.into()),
        );
        Self { error }
    }

    pub fn into_runtime_error(self) -> RuntimeError {
        self.error
    }
}

impl std::fmt::Display for PluginHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.error.route, self.error.code)
    }
}

impl std::error::Error for PluginHostError {}

pub type PluginResult<T> = Result<T, PluginHostError>;

pub fn plugin_error(route: impl Into<String>, detail: impl Into<String>) -> PluginHostError {
    PluginHostError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "mutsuki.plugin",
        route,
        detail,
    )
}

/// Optional task surface exposed to a plugin connection.
pub trait PluginTaskGateway: Send + Sync {
    fn submit_batch(&self, batch: TaskBatch) -> PluginResult<Vec<TaskHandle>>;
    fn cancel_task(&self, handle: &TaskHandle) -> PluginResult<()>;
    /// Returns a retained terminal outcome, or `None` while the registered task is non-terminal.
    ///
    /// A gateway with bounded outcome retention must return `ERR_TASK_EXPIRED` after evicting a
    /// known terminal outcome and `ERR_TASK_NOT_FOUND` for a handle it cannot identify.
    fn task_outcome(&self, handle: &TaskHandle) -> PluginResult<Option<TaskOutcome>>;
}

/// Optional resource surface exposed to a plugin connection.
pub trait PluginResourceGateway: Send + Sync {
    fn collect_read_plan(
        &self,
        provider_id: Option<&str>,
        plan: &ReadPlan,
    ) -> PluginResult<Vec<u8>>;
    fn snapshot_read_plan(
        &self,
        provider_id: Option<&str>,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> PluginResult<SnapshotDescriptor>;
    fn open_stream_plan(
        &self,
        provider_id: Option<&str>,
        plan: &ReadPlan,
    ) -> PluginResult<StreamPlan>;
    fn execute_export_plan(
        &self,
        provider_id: Option<&str>,
        plan: &ExportPlan,
    ) -> PluginResult<PlanReceipt>;
    fn commit_write_plan(
        &self,
        provider_id: Option<&str>,
        plan: &mutsuki_runtime_contracts::WritePlan,
        bytes: Vec<u8>,
    ) -> PluginResult<PlanReceipt>;
    fn execute_command_plan(
        &self,
        provider_id: Option<&str>,
        plan: &CommandPlan,
    ) -> PluginResult<PlanReceipt>;
    fn execute_command_batch(
        &self,
        provider_id: Option<&str>,
        batch: &CommandBatch,
    ) -> PluginResult<Vec<PlanReceipt>>;
    fn execute_saga_plan(
        &self,
        provider_id: Option<&str>,
        saga: &SagaPlan,
    ) -> PluginResult<Vec<PlanReceipt>>;
    fn create_blob_resource(
        &self,
        provider_id: Option<&str>,
        schema: &str,
        bytes: Vec<u8>,
    ) -> PluginResult<ResourceRef>;
    fn create_cow_state_resource(
        &self,
        provider_id: Option<&str>,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> PluginResult<ResourceRef>;
    fn create_capability_resource(
        &self,
        provider_id: Option<&str>,
        kind_id: &str,
        schema: &str,
    ) -> PluginResult<ResourceRef>;
}

/// Optional host surfaces for one plugin session.
#[derive(Clone, Default)]
pub struct PluginHostContext {
    pub task_gateway: Option<Arc<dyn PluginTaskGateway>>,
    pub resource_gateway: Option<Arc<dyn PluginResourceGateway>>,
}

impl PluginHostContext {
    pub fn with_task_gateway(mut self, gateway: Arc<dyn PluginTaskGateway>) -> Self {
        self.task_gateway = Some(gateway);
        self
    }

    pub fn with_resource_gateway(mut self, gateway: Arc<dyn PluginResourceGateway>) -> Self {
        self.resource_gateway = Some(gateway);
        self
    }

    pub fn dispatch_binary_request(&self, request: &[u8]) -> Vec<u8> {
        let decoded =
            match decode_binary_any_request(request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS) {
                Ok(decoded) => decoded,
                Err(_) => return Vec::new(),
            };
        dispatch_decoded(self, decoded)
    }
}

fn capability_missing(route: &str, capability: &str) -> PluginHostError {
    let mut error = plugin_error(
        route,
        format!("host capability is unavailable: {capability}"),
    );
    error.error.code = "plugin.capability_unavailable".to_string();
    error.error.lost_capability = Some(capability.to_string());
    error.error.recovery = Some("provide the optional gateway before loading the plugin".into());
    error
}

fn encode_result<T: serde::Serialize>(
    request_id: u64,
    opcode: Opcode,
    result: PluginResult<T>,
) -> Vec<u8> {
    let encoded = match result {
        Ok(value) => encode_binary_response(
            request_id,
            opcode,
            Ok(&value),
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        ),
        Err(error) => encode_binary_response::<()>(
            request_id,
            opcode,
            Err(&error.error),
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        ),
    };
    encoded.unwrap_or_default()
}

fn dispatch_decoded(context: &PluginHostContext, decoded: DecodedWireRequest) -> Vec<u8> {
    let request_id = decoded.request_id;
    match decoded.request {
        AnyWireRequest::SubmitTaskBatch(request) => {
            let result = context
                .task_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.task.submit", "task.submit"))
                .and_then(|gateway| gateway.submit_batch(request.batch));
            encode_result(request_id, Opcode::TaskSubmitBatch, result)
        }
        AnyWireRequest::CancelTask(request) => {
            let result = context
                .task_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.task.cancel", "task.cancel"))
                .and_then(|gateway| gateway.cancel_task(&request.handle));
            encode_result(request_id, Opcode::TaskCancel, result)
        }
        AnyWireRequest::TaskOutcome(request) => {
            let result = context
                .task_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.task.outcome", "task.outcome"))
                .and_then(|gateway| gateway.task_outcome(&request.handle));
            encode_result(request_id, Opcode::TaskOutcome, result)
        }
        AnyWireRequest::CollectReadPlan(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.resource.read", "resource.read"))
                .and_then(|gateway| {
                    gateway.collect_read_plan(request.provider_id.as_deref(), &request.plan)
                });
            encode_result(request_id, Opcode::ResourceReadCollect, result)
        }
        AnyWireRequest::SnapshotReadPlan(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.resource.snapshot", "resource.snapshot"))
                .and_then(|gateway| {
                    gateway.snapshot_read_plan(
                        request.provider_id.as_deref(),
                        &request.plan,
                        &request.kind_id,
                        &request.schema,
                    )
                });
            encode_result(request_id, Opcode::ResourceReadSnapshot, result)
        }
        AnyWireRequest::OpenStreamPlan(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.resource.stream", "resource.stream"))
                .and_then(|gateway| {
                    gateway.open_stream_plan(request.provider_id.as_deref(), &request.plan)
                });
            encode_result(request_id, Opcode::ResourceStreamOpen, result)
        }
        AnyWireRequest::ExportPlan(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.resource.export", "resource.export"))
                .and_then(|gateway| {
                    gateway.execute_export_plan(request.provider_id.as_deref(), &request.plan)
                });
            encode_result(request_id, Opcode::ResourceExport, result)
        }
        AnyWireRequest::CommitWritePlan(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.resource.write", "resource.write"))
                .and_then(|gateway| {
                    gateway.commit_write_plan(
                        request.provider_id.as_deref(),
                        &request.plan,
                        request.bytes,
                    )
                });
            encode_result(request_id, Opcode::ResourceWriteCommit, result)
        }
        AnyWireRequest::CommandPlan(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.resource.command", "resource.command"))
                .and_then(|gateway| {
                    gateway.execute_command_plan(request.provider_id.as_deref(), &request.plan)
                });
            encode_result(request_id, Opcode::ResourceCommand, result)
        }
        AnyWireRequest::CommandBatch(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| {
                    capability_missing("plugin.resource.command_batch", "resource.command_batch")
                })
                .and_then(|gateway| {
                    gateway.execute_command_batch(request.provider_id.as_deref(), &request.batch)
                });
            encode_result(request_id, Opcode::ResourceCommandBatch, result)
        }
        AnyWireRequest::SagaPlan(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| capability_missing("plugin.resource.saga", "resource.saga"))
                .and_then(|gateway| {
                    gateway.execute_saga_plan(request.provider_id.as_deref(), &request.saga)
                });
            encode_result(request_id, Opcode::ResourceSaga, result)
        }
        AnyWireRequest::CreateBlob(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| {
                    capability_missing("plugin.resource.create_blob", "resource.create_blob")
                })
                .and_then(|gateway| {
                    gateway.create_blob_resource(
                        request.provider_id.as_deref(),
                        &request.schema,
                        request.bytes,
                    )
                });
            encode_result(request_id, Opcode::ResourceCreateBlob, result)
        }
        AnyWireRequest::CreateCowState(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| {
                    capability_missing(
                        "plugin.resource.create_cow_state",
                        "resource.create_cow_state",
                    )
                })
                .and_then(|gateway| {
                    gateway.create_cow_state_resource(
                        request.provider_id.as_deref(),
                        &request.kind_id,
                        &request.schema,
                        request.bytes,
                    )
                });
            encode_result(request_id, Opcode::ResourceCreateCowState, result)
        }
        AnyWireRequest::CreateCapability(request) => {
            let result = context
                .resource_gateway
                .as_ref()
                .ok_or_else(|| {
                    capability_missing(
                        "plugin.resource.create_capability",
                        "resource.create_capability",
                    )
                })
                .and_then(|gateway| {
                    gateway.create_capability_resource(
                        request.provider_id.as_deref(),
                        &request.kind_id,
                        &request.schema,
                    )
                });
            encode_result(request_id, Opcode::ResourceCreateCapability, result)
        }
        unsupported => encode_result::<()>(
            request_id,
            unsupported.opcode(),
            Err(plugin_error(
                "plugin.host_opcode_unsupported",
                format!(
                    "unsupported host opcode {:#06x}",
                    unsupported.opcode() as u16
                ),
            )),
        ),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AbiBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

impl AbiBuffer {
    pub const fn empty() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
        }
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::empty();
        }
        let mut boxed = bytes.into_boxed_slice();
        let value = Self {
            ptr: boxed.as_mut_ptr(),
            len: boxed.len(),
        };
        std::mem::forget(boxed);
        value
    }

    /// # Safety
    /// The pointer/length pair must have been returned by a valid ABI peer.
    pub unsafe fn as_slice<'a>(&self) -> &'a [u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: the caller guarantees `ptr`/`len` came from an ABI peer, which only ever
        // produces them through `from_bytes` (a leaked `Box<[u8]>`). The zero-length case
        // returned above is the only one where `ptr` may dangle.
        unsafe { std::slice::from_raw_parts(self.ptr.cast_const(), self.len) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AbiCallResult {
    pub status: i32,
    pub payload: AbiBuffer,
}

impl AbiCallResult {
    pub fn ok(bytes: Vec<u8>) -> Self {
        Self {
            status: 0,
            payload: AbiBuffer::from_bytes(bytes),
        }
    }
    pub fn failed(bytes: Vec<u8>) -> Self {
        Self {
            status: 1,
            payload: AbiBuffer::from_bytes(bytes),
        }
    }
}

pub type AbiRequestFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> AbiCallResult;
pub type AbiReleaseFn = unsafe extern "C" fn(AbiBuffer);
pub type AbiCloseFn = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiHostV2 {
    pub context: *mut c_void,
    pub request: Option<AbiRequestFn>,
    pub release: Option<AbiReleaseFn>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiPluginV2 {
    pub transport_version: u32,
    pub context: *mut c_void,
    pub request: Option<AbiRequestFn>,
    pub release: Option<AbiReleaseFn>,
    pub close: Option<AbiCloseFn>,
}

pub type AbiEntryV2 = unsafe extern "C" fn(AbiHostV2) -> AbiPluginV2;

// SAFETY: `AbiHostV2` is a plain `#[repr(C)]` bundle of an opaque host context pointer and
// two `extern "C"` function pointers. The host contract requires those callbacks to be
// callable from any thread and to serialize access to the context internally, so moving and
// sharing the bundle carries no additional aliasing obligation of its own.
unsafe impl Send for AbiHostV2 {}
unsafe impl Sync for AbiHostV2 {}

/// A plugin-side ABI v2 endpoint.
///
/// `request` takes `&self` and the trait requires `Sync` because the host may have several calls
/// in flight at once. Wrapping the whole guest in one mutex here would push every plugin back to
/// one request at a time no matter how the host schedules them, so the synchronisation belongs
/// inside the guest, where it can be scoped to the individual runner or provider a request
/// actually touches.
pub trait AbiGuest: Send + Sync {
    fn request(&self, request: &[u8]) -> Vec<u8>;
}

pub fn plugin_api_from_guest(guest: Box<dyn AbiGuest>) -> AbiPluginV2 {
    let context = Box::into_raw(Box::new(guest)).cast::<c_void>();
    AbiPluginV2 {
        transport_version: ABI_V2_TRANSPORT_VERSION,
        context,
        request: Some(guest_request),
        release: Some(release_buffer),
        close: Some(close_guest),
    }
}

unsafe extern "C" fn guest_request(
    context: *mut c_void,
    request: *const u8,
    request_len: usize,
) -> AbiCallResult {
    if context.is_null() || (request.is_null() && request_len != 0) {
        return AbiCallResult::failed(b"invalid ABI request pointers".to_vec());
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let request = if request_len == 0 {
            &[]
        } else {
            // SAFETY: the null/length combinations rejected above leave only a non-null
            // pointer to `request_len` initialised bytes owned by the caller. The borrow
            // ends before this function returns, so it cannot outlive the caller's buffer.
            unsafe { std::slice::from_raw_parts(request, request_len) }
        };
        // SAFETY: `context` was checked non-null above and is only ever produced by
        // `plugin_api_from_guest`, which leaks exactly this type. The host keeps it alive
        // until it calls `close_guest`, and `AbiGuest: Sync` makes the shared reference sound
        // across the host threads that may call in concurrently.
        let guest = unsafe { &*(context.cast::<Box<dyn AbiGuest>>()) };
        guest.request(request)
    }));
    match result {
        Ok(response) => AbiCallResult::ok(response),
        Err(_) => AbiCallResult::failed(b"ABI guest panicked".to_vec()),
    }
}

unsafe extern "C" fn release_buffer(buffer: AbiBuffer) {
    if buffer.ptr.is_null() || buffer.len == 0 {
        return;
    }
    let slice = ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len);
    // SAFETY: non-empty buffers reaching this point were built by `AbiBuffer::from_bytes`,
    // which leaks a `Box<[u8]>` of exactly `len` elements. The ABI contract makes release
    // single-shot, so reconstructing and dropping that box here is the matching free.
    unsafe {
        drop(Box::from_raw(slice));
    }
}

unsafe extern "C" fn close_guest(context: *mut c_void) {
    if !context.is_null() {
        // SAFETY: `context` is the pointer leaked by `plugin_api_from_guest` and the ABI
        // contract calls `close` at most once, after the last `request` has returned.
        unsafe {
            drop(Box::from_raw(context.cast::<Box<dyn AbiGuest>>()));
        }
    }
}

pub fn consume_call_result(
    result: AbiCallResult,
    release: Option<AbiReleaseFn>,
    route: impl Into<String>,
) -> PluginResult<(bool, Vec<u8>)> {
    let valid_buffer = (result.payload.len == 0 && result.payload.ptr.is_null())
        || (result.payload.len > 0 && !result.payload.ptr.is_null());
    if !valid_buffer {
        return Err(plugin_error(route, "invalid payload pointer/length pair"));
    }
    if result.payload.len > 0 && release.is_none() {
        return Err(plugin_error(
            route,
            "non-empty payload is missing its release callback",
        ));
    }
    // SAFETY: the pointer/length pair was validated as a consistent pair just above, so it is
    // either empty or a live ABI-owned allocation. The copy completes before the buffer is
    // handed back to its owner for release.
    let bytes = unsafe { result.payload.as_slice() }.to_vec();
    if result.payload.len > 0 {
        // SAFETY: a non-empty payload was proven to carry a release callback above, and this
        // is the single release call for that buffer.
        unsafe {
            release.expect("validated release callback")(result.payload);
        }
    }
    match result.status {
        0 => Ok((true, bytes)),
        1 => Ok((false, bytes)),
        _ => Err(plugin_error(route, "invalid ABI callback status")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::{
        CancelPolicy, ERR_TASK_EXPIRED, ERR_TASK_NOT_FOUND, Task, TaskBatch,
    };
    use mutsuki_runtime_wire::{
        SubmitTaskBatchRequest, TaskOutcomeRequest, decode_binary_response, encode_binary_request,
    };

    struct OutcomeGateway;

    impl PluginTaskGateway for OutcomeGateway {
        fn submit_batch(&self, _batch: TaskBatch) -> PluginResult<Vec<TaskHandle>> {
            unreachable!("outcome contract test does not submit tasks")
        }

        fn cancel_task(&self, _handle: &TaskHandle) -> PluginResult<()> {
            unreachable!("outcome contract test does not cancel tasks")
        }

        fn task_outcome(&self, handle: &TaskHandle) -> PluginResult<Option<TaskOutcome>> {
            match handle.task_id.as_str() {
                "running" => Ok(None),
                "expired" => Err(PluginHostError::new(
                    ERR_TASK_EXPIRED,
                    "plugin.test",
                    "plugin.task.outcome",
                    "terminal outcome was evicted",
                )),
                _ => Err(PluginHostError::new(
                    ERR_TASK_NOT_FOUND,
                    "plugin.test",
                    "plugin.task.outcome",
                    "task handle is unknown",
                )),
            }
        }
    }

    fn task_handle(task_id: &str) -> TaskHandle {
        TaskHandle {
            task_id: task_id.into(),
            protocol_id: "demo.task".into(),
            target_binding_id: None,
            cancel_policy: CancelPolicy::Cascade,
            trace_id: None,
            correlation_id: None,
        }
    }

    fn dispatch_outcome(
        context: &PluginHostContext,
        request_id: u64,
        task_id: &str,
    ) -> Result<Option<TaskOutcome>, RuntimeError> {
        let request = TaskOutcomeRequest {
            handle: task_handle(task_id),
        };
        let frame = encode_binary_request(
            request_id,
            &request,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
        .expect("request should encode");
        let response = context.dispatch_binary_request(&frame);
        decode_binary_response::<TaskOutcomeRequest>(
            &response,
            request_id,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
    }

    #[test]
    fn missing_task_gateway_is_a_structured_capability_error() {
        let request = SubmitTaskBatchRequest {
            batch: TaskBatch::one(
                "plugin-test.batch",
                Task::new("task-1", "demo.task", serde_json::json!({})),
            ),
        };
        let frame = encode_binary_request(7, &request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
            .expect("request should encode");
        let response = PluginHostContext::default().dispatch_binary_request(&frame);
        let error = decode_binary_response::<SubmitTaskBatchRequest>(
            &response,
            7,
            mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
        )
        .expect_err("missing task gateway should fail");
        assert_eq!(error.code, "plugin.capability_unavailable");
        assert_eq!(error.lost_capability.as_deref(), Some("task.submit"));
    }

    #[test]
    fn task_outcome_dispatch_distinguishes_running_expired_and_unknown_handles() {
        let context = PluginHostContext::default().with_task_gateway(Arc::new(OutcomeGateway));

        assert_eq!(dispatch_outcome(&context, 10, "running").unwrap(), None);
        assert_eq!(
            dispatch_outcome(&context, 11, "expired")
                .expect_err("evicted outcome should be expired")
                .code,
            ERR_TASK_EXPIRED
        );
        assert_eq!(
            dispatch_outcome(&context, 12, "unknown")
                .expect_err("unregistered handle should be unknown")
                .code,
            ERR_TASK_NOT_FOUND
        );
    }
}
