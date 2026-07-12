use std::collections::BTreeMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Mutex;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PluginManifest, ReadPlan, RuntimeError, TaskBatch, TaskHandle,
    TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{LoadedPlugin, ResourcePlanGateway, ResourceProviderGateway, TaskSubmitter};

pub const ABI_TRANSPORT_VERSION: u32 = 1;
pub const ABI_ENTRY_SYMBOL: &[u8] = b"mutsuki_plugin_abi_v1\0";
pub const ABI_CODEC_ID: &str = "mutsuki.codec.jsonl.v1";
pub const ABI_BRIDGE_ID: &str = "mutsuki.bridge.abi.jsonl.v1";

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
        let buffer = Self {
            ptr: boxed.as_mut_ptr(),
            len: boxed.len(),
        };
        std::mem::forget(boxed);
        buffer
    }

    /// # Safety
    ///
    /// The buffer must remain valid for the returned slice and originate from the owner that
    /// supplied the matching release callback.
    pub unsafe fn as_slice<'a>(&self) -> &'a [u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: guaranteed by the caller of this unsafe function.
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
pub struct AbiHostV1 {
    pub context: *mut c_void,
    pub request: Option<AbiRequestFn>,
    pub release: Option<AbiReleaseFn>,
}

// The host owns the callback context and guarantees it remains valid for the plugin connection.
unsafe impl Send for AbiHostV1 {}
unsafe impl Sync for AbiHostV1 {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiPluginV1 {
    pub transport_version: u32,
    pub context: *mut c_void,
    pub request: Option<AbiRequestFn>,
    pub release: Option<AbiReleaseFn>,
    pub close: Option<AbiCloseFn>,
}

pub type AbiEntryV1 = unsafe extern "C" fn(AbiHostV1) -> AbiPluginV1;

pub trait AbiGuest: Send {
    fn request(&mut self, request: &[u8]) -> Vec<u8>;
}

pub fn dispatch_host_request(
    task_submitter: &dyn TaskSubmitter,
    resource_gateway: &dyn ResourcePlanGateway,
    request: &[u8],
) -> Vec<u8> {
    let parsed: Result<Value, _> = serde_json::from_slice(request);
    let response = match parsed {
        Ok(request) => {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let method = request.get("method").and_then(Value::as_str);
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let result = match method {
                Some("task.submit_batch") => decode_field(&params, "batch")
                    .and_then(|batch| task_submitter.submit_batch(batch))
                    .and_then(encode_value),
                Some("task.cancel") => decode_field(&params, "handle")
                    .and_then(|handle| task_submitter.cancel_task(&handle))
                    .map(|()| Value::Null),
                Some("task.outcome") => decode_field(&params, "handle")
                    .and_then(|handle| task_submitter.task_outcome(&handle))
                    .and_then(encode_value),
                Some("resource.read.collect") => decode_field(&params, "plan")
                    .and_then(|plan| resource_gateway.collect_read_plan(&plan))
                    .and_then(encode_value),
                Some("resource.read.snapshot") => {
                    let plan = decode_field(&params, "plan");
                    let kind_id = required_str(&params, "kind_id");
                    let schema = required_str(&params, "schema");
                    match (plan, kind_id, schema) {
                        (Ok(plan), Ok(kind_id), Ok(schema)) => resource_gateway
                            .snapshot_read_plan(&plan, kind_id, schema)
                            .and_then(encode_value),
                        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
                    }
                }
                Some("resource.stream.open") => decode_field(&params, "plan")
                    .and_then(|plan| resource_gateway.open_stream_plan(&plan))
                    .and_then(encode_value),
                Some("resource.export") => decode_field(&params, "plan")
                    .and_then(|plan| resource_gateway.execute_export_plan(&plan))
                    .and_then(encode_value),
                Some("resource.write.commit") => {
                    let plan = decode_field(&params, "plan");
                    let bytes = decode_field(&params, "bytes");
                    match (plan, bytes) {
                        (Ok(plan), Ok(bytes)) => resource_gateway
                            .commit_write_plan(&plan, bytes)
                            .and_then(encode_value),
                        (Err(error), _) | (_, Err(error)) => Err(error),
                    }
                }
                Some("resource.command") => decode_field(&params, "plan")
                    .and_then(|plan| resource_gateway.execute_command_plan(&plan))
                    .and_then(encode_value),
                Some("resource.command_batch") => decode_field(&params, "batch")
                    .and_then(|batch| resource_gateway.execute_command_batch(&batch))
                    .and_then(encode_value),
                Some("resource.saga") => decode_field(&params, "saga")
                    .and_then(|saga| resource_gateway.execute_saga_plan(&saga))
                    .and_then(encode_value),
                Some(method) => Err(abi_failure(
                    "abi.host_method_unsupported",
                    format!("unsupported host method {method}"),
                )),
                None => Err(abi_failure("abi.method_missing", "missing method")),
            };
            match result {
                Ok(result) => json!({ "id": id, "ok": true, "result": result }),
                Err(error) => json!({ "id": id, "ok": false, "error": error.error() }),
            }
        }
        Err(error) => json!({
            "id": null,
            "ok": false,
            "error": RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "abi.host",
                format!("abi.decode:{error}"),
            ),
        }),
    };
    serde_json::to_vec(&response).expect("ABI host response must serialize")
}

#[derive(Clone, Copy)]
pub struct AbiHostClient {
    host: AbiHostV1,
}

impl AbiHostClient {
    pub fn new(host: AbiHostV1) -> Self {
        Self { host }
    }

    fn request_value(&self, method: &str, params: Value) -> RuntimeResult<Value> {
        let request = self.host.request.ok_or_else(|| {
            abi_failure(
                "abi.host_callback_missing",
                "host request callback is unavailable",
            )
        })?;
        let bytes = serde_json::to_vec(&json!({
            "id": "guest-1",
            "method": method,
            "params": params,
        }))
        .map_err(|error| abi_failure("abi.encode", error.to_string()))?;
        // SAFETY: the host owns the callback and keeps context valid for this connection.
        let response = unsafe { request(self.host.context, bytes.as_ptr(), bytes.len()) };
        // SAFETY: response bytes remain valid until the host release callback is invoked.
        let response_bytes = unsafe { response.payload.as_slice() }.to_vec();
        if let Some(release) = self.host.release {
            // SAFETY: this is the release function paired with the host-owned response buffer.
            unsafe { release(response.payload) };
        }
        if response.status != 0 {
            return Err(abi_failure(
                "abi.host_callback_failed",
                String::from_utf8_lossy(&response_bytes),
            ));
        }
        decode_response(&response_bytes, "guest-1")
    }

    fn request_as<T: DeserializeOwned>(&self, method: &str, params: Value) -> RuntimeResult<T> {
        serde_json::from_value(self.request_value(method, params)?)
            .map_err(|error| abi_failure("abi.decode", error.to_string()))
    }
}

impl TaskSubmitter for AbiHostClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.request_as("task.submit_batch", json!({ "batch": batch }))
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.request_value("task.cancel", json!({ "handle": handle }))?;
        Ok(())
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.request_as("task.outcome", json!({ "handle": handle }))
    }
}

impl ResourcePlanGateway for AbiHostClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.request_as("resource.read.collect", json!({ "plan": plan }))
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<mutsuki_runtime_contracts::SnapshotDescriptor> {
        self.request_as(
            "resource.read.snapshot",
            json!({ "plan": plan, "kind_id": kind_id, "schema": schema }),
        )
    }

    fn open_stream_plan(
        &self,
        plan: &ReadPlan,
    ) -> RuntimeResult<mutsuki_runtime_contracts::StreamPlan> {
        self.request_as("resource.stream.open", json!({ "plan": plan }))
    }

    fn execute_export_plan(
        &self,
        plan: &ExportPlan,
    ) -> RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        self.request_as("resource.export", json!({ "plan": plan }))
    }

    fn commit_write_plan(
        &self,
        plan: &WritePlan,
        bytes: Vec<u8>,
    ) -> RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        self.request_as(
            "resource.write.commit",
            json!({ "plan": plan, "bytes": bytes }),
        )
    }

    fn execute_command_plan(
        &self,
        plan: &CommandPlan,
    ) -> RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        self.request_as("resource.command", json!({ "plan": plan }))
    }

    fn execute_command_batch(
        &self,
        batch: &CommandBatch,
    ) -> RuntimeResult<Vec<mutsuki_runtime_contracts::PlanReceipt>> {
        self.request_as("resource.command_batch", json!({ "batch": batch }))
    }

    fn execute_saga_plan(
        &self,
        saga: &SagaPlan,
    ) -> RuntimeResult<Vec<mutsuki_runtime_contracts::PlanReceipt>> {
        self.request_as("resource.saga", json!({ "saga": saga }))
    }
}

pub struct JsonlPluginGuest {
    manifest: PluginManifest,
    runners: BTreeMap<String, Box<dyn Runner>>,
    providers: BTreeMap<String, std::sync::Arc<dyn ResourceProviderGateway>>,
}

impl JsonlPluginGuest {
    pub fn new(plugin: LoadedPlugin) -> RuntimeResult<Self> {
        if !plugin.host_services.is_empty() {
            return Err(abi_failure(
                "abi.host_service_unsupported",
                "ABI plugins cannot export host services",
            ));
        }
        let mut runners = BTreeMap::new();
        for runner in plugin.runners {
            let runner_id = runner.descriptor().runner_id.clone();
            if runners.insert(runner_id.clone(), runner).is_some() {
                return Err(abi_failure(
                    "abi.runner_duplicate",
                    format!("duplicate runner {runner_id}"),
                ));
            }
        }
        let mut providers = BTreeMap::new();
        for provider in plugin.resource_providers {
            if providers
                .insert(provider.provider_id.clone(), provider.provider)
                .is_some()
            {
                return Err(abi_failure(
                    "abi.provider_duplicate",
                    format!("duplicate resource provider {}", provider.provider_id),
                ));
            }
        }
        Ok(Self {
            manifest: plugin.manifest,
            runners,
            providers,
        })
    }

    fn dispatch(&mut self, method: &str, params: Value) -> RuntimeResult<Value> {
        match method {
            "plugin.handshake" => Ok(json!({
                "transport_version": ABI_TRANSPORT_VERSION,
                "codec_id": ABI_CODEC_ID,
                "bridge_id": ABI_BRIDGE_ID,
                "manifest": self.manifest,
                "resource_provider_ids": self.providers.keys().collect::<Vec<_>>(),
            })),
            "runner.run_batch" => {
                let runner_id = required_str(&params, "runner_id")?;
                let ctx = decode_field(&params, "ctx")?;
                let batch = decode_field(&params, "batch")?;
                let runner = self.runners.get_mut(runner_id).ok_or_else(|| {
                    abi_failure("abi.runner_not_found", format!("runner {runner_id}"))
                })?;
                serde_json::to_value(runner.run_batch(ctx, batch)?)
                    .map_err(|error| abi_failure("abi.encode", error.to_string()))
            }
            "runner.cancel" => {
                let runner_id = required_str(&params, "runner_id")?;
                let invocation_id = required_str(&params, "invocation_id")?;
                self.runners
                    .get_mut(runner_id)
                    .ok_or_else(|| abi_failure("abi.runner_not_found", runner_id))?
                    .cancel(invocation_id)?;
                Ok(Value::Null)
            }
            "runner.dispose" => {
                let runner_id = required_str(&params, "runner_id")?;
                self.runners
                    .get_mut(runner_id)
                    .ok_or_else(|| abi_failure("abi.runner_not_found", runner_id))?
                    .dispose()?;
                Ok(Value::Null)
            }
            "resource.create_blob" => {
                let provider = self.provider(&params)?;
                let schema = required_str(&params, "schema")?;
                let bytes = decode_field(&params, "bytes")?;
                encode_value(provider.create_blob_resource(schema, bytes)?)
            }
            "resource.create_cow_state" => {
                let provider = self.provider(&params)?;
                let kind_id = required_str(&params, "kind_id")?;
                let schema = required_str(&params, "schema")?;
                let bytes = decode_field(&params, "bytes")?;
                encode_value(provider.create_cow_state_resource(kind_id, schema, bytes)?)
            }
            "resource.create_capability" => {
                let provider = self.provider(&params)?;
                encode_value(provider.create_capability_resource(
                    required_str(&params, "kind_id")?,
                    required_str(&params, "schema")?,
                )?)
            }
            "resource.read.collect" => {
                let provider = self.provider(&params)?;
                encode_value(provider.collect_read_plan(&decode_field(&params, "plan")?)?)
            }
            "resource.read.snapshot" => {
                let provider = self.provider(&params)?;
                encode_value(provider.snapshot_read_plan(
                    &decode_field(&params, "plan")?,
                    required_str(&params, "kind_id")?,
                    required_str(&params, "schema")?,
                )?)
            }
            "resource.stream.open" => {
                let provider = self.provider(&params)?;
                encode_value(provider.open_stream_plan(&decode_field(&params, "plan")?)?)
            }
            "resource.export" => {
                let provider = self.provider(&params)?;
                encode_value(provider.execute_export_plan(&decode_field(&params, "plan")?)?)
            }
            "resource.write.commit" => {
                let provider = self.provider(&params)?;
                encode_value(provider.commit_write_plan(
                    &decode_field(&params, "plan")?,
                    decode_field(&params, "bytes")?,
                )?)
            }
            "resource.command" => {
                let provider = self.provider(&params)?;
                encode_value(provider.execute_command_plan(&decode_field(&params, "plan")?)?)
            }
            "resource.command_batch" => {
                let provider = self.provider(&params)?;
                encode_value(provider.execute_command_batch(&decode_field(&params, "batch")?)?)
            }
            "resource.saga" => {
                let provider = self.provider(&params)?;
                encode_value(provider.execute_saga_plan(&decode_field(&params, "saga")?)?)
            }
            _ => Err(abi_failure(
                "abi.method_unsupported",
                format!("unsupported method {method}"),
            )),
        }
    }

    fn provider(
        &self,
        params: &Value,
    ) -> RuntimeResult<&std::sync::Arc<dyn ResourceProviderGateway>> {
        let provider_id = required_str(params, "provider_id")?;
        self.providers.get(provider_id).ok_or_else(|| {
            abi_failure(
                "abi.provider_not_found",
                format!("resource provider {provider_id}"),
            )
        })
    }
}

impl AbiGuest for JsonlPluginGuest {
    fn request(&mut self, request: &[u8]) -> Vec<u8> {
        let parsed: Result<Value, _> = serde_json::from_slice(request);
        let response = match parsed {
            Ok(request) => {
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                let method = request.get("method").and_then(Value::as_str);
                let params = request.get("params").cloned().unwrap_or(Value::Null);
                match method {
                    Some(method) => match self.dispatch(method, params) {
                        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
                        Err(error) => {
                            json!({ "id": id, "ok": false, "error": error.error() })
                        }
                    },
                    None => json!({
                        "id": id,
                        "ok": false,
                        "error": RuntimeError::new(
                            mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                            "abi.guest",
                            "abi.method_missing",
                        ),
                    }),
                }
            }
            Err(error) => json!({
                "id": null,
                "ok": false,
                "error": RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                    "abi.guest",
                    format!("abi.decode:{error}"),
                ),
            }),
        };
        serde_json::to_vec(&response).expect("ABI JSON response must serialize")
    }
}

pub fn plugin_api_from_guest(guest: Box<dyn AbiGuest>) -> AbiPluginV1 {
    let context = Box::into_raw(Box::new(Mutex::new(guest))).cast::<c_void>();
    AbiPluginV1 {
        transport_version: ABI_TRANSPORT_VERSION,
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
        // SAFETY: pointers are validated above and owned by the caller for this call.
        let request = unsafe { std::slice::from_raw_parts(request, request_len) };
        // SAFETY: context was created by plugin_api_from_guest and remains owned by this API.
        let guest = unsafe { &*(context.cast::<Mutex<Box<dyn AbiGuest>>>()) };
        guest
            .lock()
            .expect("ABI guest mutex poisoned")
            .request(request)
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
    // SAFETY: buffers returned by this module are allocated as Box<[u8]> with this exact length.
    unsafe { drop(Box::from_raw(slice)) };
}

unsafe extern "C" fn close_guest(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    // SAFETY: context is uniquely returned by plugin_api_from_guest and closed once by the host.
    unsafe { drop(Box::from_raw(context.cast::<Mutex<Box<dyn AbiGuest>>>())) };
}

#[macro_export]
macro_rules! export_mutsuki_plugin_abi_v1 {
    ($factory:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn mutsuki_plugin_abi_v1(
            host: $crate::abi::AbiHostV1,
        ) -> $crate::abi::AbiPluginV1 {
            let host_client = $crate::abi::AbiHostClient::new(host);
            let guest: Box<dyn $crate::abi::AbiGuest> = match $factory(host_client) {
                Ok(plugin) => match $crate::abi::JsonlPluginGuest::new(plugin) {
                    Ok(guest) => Box::new(guest),
                    Err(error) => Box::new($crate::abi::FailedAbiGuest::new(error)),
                },
                Err(error) => Box::new($crate::abi::FailedAbiGuest::new(error)),
            };
            $crate::abi::plugin_api_from_guest(guest)
        }
    };
}

pub struct FailedAbiGuest {
    error: RuntimeError,
}

impl FailedAbiGuest {
    pub fn new(error: RuntimeFailure) -> Self {
        Self {
            error: error.error().clone(),
        }
    }
}

impl AbiGuest for FailedAbiGuest {
    fn request(&mut self, request: &[u8]) -> Vec<u8> {
        let id = serde_json::from_slice::<Value>(request)
            .ok()
            .and_then(|value| value.get("id").cloned())
            .unwrap_or(Value::Null);
        serde_json::to_vec(&json!({ "id": id, "ok": false, "error": self.error }))
            .expect("ABI failure response must serialize")
    }
}

fn required_str<'a>(value: &'a Value, field: &str) -> RuntimeResult<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| abi_failure("abi.field_missing", format!("missing string field {field}")))
}

fn decode_field<T: DeserializeOwned>(value: &Value, field: &str) -> RuntimeResult<T> {
    serde_json::from_value(value.get(field).cloned().unwrap_or(Value::Null))
        .map_err(|error| abi_failure("abi.decode", format!("field {field}: {error}")))
}

fn encode_value(value: impl serde::Serialize) -> RuntimeResult<Value> {
    serde_json::to_value(value).map_err(|error| abi_failure("abi.encode", error.to_string()))
}

fn decode_response(bytes: &[u8], expected_id: &str) -> RuntimeResult<Value> {
    let response: Value = serde_json::from_slice(bytes)
        .map_err(|error| abi_failure("abi.decode", error.to_string()))?;
    if response.get("id") != Some(&Value::String(expected_id.into())) {
        return Err(abi_failure("abi.response_id_mismatch", expected_id));
    }
    match response.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(response.get("result").cloned().unwrap_or(Value::Null)),
        Some(false) => {
            let error =
                serde_json::from_value(response.get("error").cloned().unwrap_or(Value::Null))
                    .map_err(|error| abi_failure("abi.decode", error.to_string()))?;
            Err(RuntimeFailure::new(error))
        }
        None => Err(abi_failure("abi.response_invalid", "missing ok field")),
    }
}

fn abi_failure(route: impl Into<String>, detail: impl Into<String>) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "mutsuki.runtime.abi",
        route,
    );
    error.evidence.insert(
        "detail".into(),
        mutsuki_runtime_contracts::ScalarValue::String(detail.into()),
    );
    RuntimeFailure::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginBuilder;
    use mutsuki_runtime_contracts::{ArtifactType, PluginArtifact, RunnerDescriptor};
    use mutsuki_runtime_core::{RunnerContext, RuntimeResult};

    struct NoopRunner(RunnerDescriptor);

    impl Runner for NoopRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.0
        }

        fn run_batch(
            &mut self,
            _ctx: RunnerContext,
            batch: mutsuki_runtime_contracts::WorkBatch,
        ) -> RuntimeResult<mutsuki_runtime_contracts::CompletionBatch> {
            Ok(mutsuki_runtime_contracts::CompletionBatch::from_results(
                &batch,
                Vec::new(),
            ))
        }
    }

    #[test]
    fn guest_handshake_uses_existing_manifest_and_jsonl_envelope() {
        let plugin = PluginBuilder::new("test.abi").build();
        let mut guest = JsonlPluginGuest::new(plugin).unwrap();
        let response = guest.request(br#"{"id":"req-1","method":"plugin.handshake","params":{}}"#);
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["id"], "req-1");
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["manifest"]["plugin_id"], "test.abi");
        assert_eq!(response["result"]["codec_id"], ABI_CODEC_ID);
    }

    #[test]
    fn owned_buffer_round_trip_uses_paired_release() {
        let buffer = AbiBuffer::from_bytes(b"payload".to_vec());
        // SAFETY: buffer is live until released below.
        assert_eq!(unsafe { buffer.as_slice() }, b"payload");
        // SAFETY: buffer was allocated by AbiBuffer::from_bytes and is released once.
        unsafe { release_buffer(buffer) };
    }

    #[test]
    fn plugin_builder_uses_final_abi_artifact_for_backend_surface() {
        let descriptor = crate::RunnerDescriptorBuilder::new("test.abi.runner", "test.abi")
            .accepted_protocol("test.abi.run")
            .build();
        let plugin = PluginBuilder::new("test.abi")
            .runner(Box::new(NoopRunner(descriptor)))
            .artifact(PluginArtifact {
                artifact_type: ArtifactType::Abi,
                path: "test_abi.dll".into(),
                sha256: "sha256:test".into(),
            })
            .build();

        assert_eq!(
            plugin.manifest.provides.plugin_backends[0].deployment_kind,
            mutsuki_runtime_contracts::PluginDeploymentKind::Abi
        );
        assert_eq!(
            plugin.manifest.provides.plugin_backends[0]
                .codec_id
                .as_deref(),
            Some(ABI_CODEC_ID)
        );
        assert_eq!(plugin.manifest.provides.bridges[0].bridge_id, ABI_BRIDGE_ID);
    }
}
