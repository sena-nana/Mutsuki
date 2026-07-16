use super::*;
use crate::{PluginBuilder, RunnerDescriptorBuilder};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, PluginArtifact, RunnerDescriptor, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::{
    BINARY_CODEC_ID, DEBUG_JSONL_CODEC_ID, DisposeRunnerRequest, InitializeRequest, ProtocolHello,
    decode_binary_response, decode_jsonl_response, encode_binary_request, encode_jsonl_request,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

static TRACKED_RELEASES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn track_release(buffer: AbiBuffer) {
    TRACKED_RELEASES.fetch_add(1, Ordering::SeqCst);
    unsafe { super::types::release_buffer(buffer) };
}

struct NoopRunner(RunnerDescriptor);

impl Runner for NoopRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.0
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_results(&batch, Vec::new()))
    }
}

#[test]
fn configured_binary_guest_initializes_with_abi_v2_codec() {
    let plugin = PluginBuilder::new("test.abi.v2").build();
    let mut guest = ConfiguredBinaryPluginGuest::new(Box::new(move |config| {
        assert_eq!(config, json!({"mode": "binary"}));
        Ok(plugin)
    }));
    let request = InitializeRequest {
        hello: ProtocolHello::binary(),
        config: Some(json!({"mode": "binary"})),
    };
    let encoded =
        encode_binary_request(7, &request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS).unwrap();

    let response = guest.request(&encoded);
    let ack = decode_binary_response::<InitializeRequest>(
        &response,
        7,
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
    )
    .unwrap();

    assert_eq!(ack.codec_id, BINARY_CODEC_ID);
    assert_eq!(
        ack.plugin
            .as_ref()
            .map(|plugin| plugin.manifest.plugin_id.as_str()),
        Some("test.abi.v2")
    );
}

#[test]
fn abi_v2_api_has_distinct_transport_version() {
    let guest: Box<dyn AbiGuest> = Box::new(FailedBinaryAbiGuest::new(RuntimeFailure::new(
        mutsuki_runtime_contracts::RuntimeError::new(
            mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
            "abi.test",
            "abi.test.failed",
        ),
    )));
    let api = plugin_api_v2_from_guest(guest);
    assert_eq!(api.transport_version, ABI_V2_TRANSPORT_VERSION);
    unsafe { api.close.unwrap()(api.context) };
}

#[test]
fn abi_v2_rejects_invalid_request_pointer_and_releases_error() {
    let guest: Box<dyn AbiGuest> = Box::new(FailedBinaryAbiGuest::new(RuntimeFailure::new(
        mutsuki_runtime_contracts::RuntimeError::new(
            mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
            "abi.test",
            "abi.test.failed",
        ),
    )));
    let api = plugin_api_v2_from_guest(guest);
    let response = unsafe { api.request.unwrap()(api.context, std::ptr::null(), 1) };

    assert_eq!(response.status, 1);
    assert_eq!(
        unsafe { response.payload.as_slice() },
        b"invalid ABI request pointers"
    );
    unsafe { api.release.unwrap()(response.payload) };
    unsafe { api.close.unwrap()(api.context) };
}

#[test]
fn abi_result_contract_rejects_bad_status_and_pointer_pairs() {
    let releases_before = TRACKED_RELEASES.load(Ordering::SeqCst);
    let error = super::types::consume_call_result(
        AbiCallResult {
            status: 7,
            payload: AbiBuffer::from_bytes(b"bad status".to_vec()),
        },
        Some(track_release),
        "abi.test.contract",
    )
    .unwrap_err();
    assert_eq!(error.error().route, "abi.test.contract");
    assert_eq!(TRACKED_RELEASES.load(Ordering::SeqCst), releases_before + 1);

    let error = super::types::consume_call_result(
        AbiCallResult {
            status: 0,
            payload: AbiBuffer {
                ptr: std::ptr::null_mut(),
                len: 1,
            },
        },
        Some(track_release),
        "abi.test.contract",
    )
    .unwrap_err();
    assert_eq!(error.error().route, "abi.test.contract");
    assert_eq!(TRACKED_RELEASES.load(Ordering::SeqCst), releases_before + 1);
}

#[test]
fn configured_guest_initializes_from_typed_request_and_returns_surface() {
    let plugin = PluginBuilder::new("test.abi").build();
    let mut guest = ConfiguredJsonlPluginGuest::new(Box::new(move |config| {
        assert_eq!(config, json!({"mode": "test"}));
        Ok(plugin)
    }));
    let request = InitializeRequest {
        hello: ProtocolHello::debug_jsonl(),
        config: Some(json!({"mode": "test"})),
    };
    let encoded =
        encode_jsonl_request(1, &request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS).unwrap();

    let response = guest.request(&encoded);
    let ack = decode_jsonl_response::<InitializeRequest>(
        &response,
        1,
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
    )
    .unwrap();

    assert_eq!(ack.codec_id, DEBUG_JSONL_CODEC_ID);
    assert_eq!(
        ack.plugin
            .as_ref()
            .map(|plugin| plugin.manifest.plugin_id.as_str()),
        Some("test.abi")
    );
}

#[test]
fn configured_guest_rejects_business_request_before_init_and_duplicate_init() {
    let mut guest =
        ConfiguredJsonlPluginGuest::new(Box::new(|_| Ok(PluginBuilder::new("test.abi").build())));
    let dispose = DisposeRunnerRequest {
        runner_id: "missing".into(),
    };
    let encoded =
        encode_jsonl_request(1, &dispose, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS).unwrap();
    let before = guest.request(&encoded);
    let error = decode_jsonl_response::<DisposeRunnerRequest>(
        &before,
        1,
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
    )
    .unwrap_err();
    assert_eq!(error.route, "abi.not_initialized");

    let initialize = InitializeRequest {
        hello: ProtocolHello::debug_jsonl(),
        config: None,
    };
    let encoded =
        encode_jsonl_request(2, &initialize, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS).unwrap();
    decode_jsonl_response::<InitializeRequest>(
        &guest.request(&encoded),
        2,
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
    )
    .unwrap();

    let duplicate =
        encode_jsonl_request(3, &initialize, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS).unwrap();
    let error = decode_jsonl_response::<InitializeRequest>(
        &guest.request(&duplicate),
        3,
        mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS,
    )
    .unwrap_err();
    assert_eq!(error.route, "abi.already_initialized");
}

#[test]
fn owned_buffer_round_trip_uses_paired_release() {
    let buffer = AbiBuffer::from_bytes(b"payload".to_vec());
    assert_eq!(unsafe { buffer.as_slice() }, b"payload");
    unsafe { super::types::release_buffer(buffer) };
}

#[test]
fn plugin_builder_declares_the_typed_jsonl_compatibility_backend() {
    let descriptor = RunnerDescriptorBuilder::new("test.abi.runner", "test.abi")
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

    let backend = &plugin.manifest.provides.plugin_backends[0];
    assert_eq!(
        backend.deployment_kind,
        mutsuki_runtime_contracts::PluginDeploymentKind::Abi
    );
    assert_eq!(backend.codec_id.as_deref(), Some(ABI_V2_CODEC_ID));
    assert_eq!(
        plugin.manifest.provides.bridges[0].bridge_id,
        ABI_V2_BRIDGE_ID
    );
}
