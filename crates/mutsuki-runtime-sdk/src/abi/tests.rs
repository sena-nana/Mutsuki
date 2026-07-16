use super::*;
use crate::{PluginBuilder, RunnerDescriptorBuilder};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, PluginArtifact, RunnerDescriptor, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_wire::{
    DEBUG_JSONL_CODEC_ID, DisposeRunnerRequest, InitializeRequest, ProtocolHello,
    decode_jsonl_response, encode_jsonl_request,
};
use serde_json::json;

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
    assert_eq!(backend.codec_id.as_deref(), Some(ABI_CODEC_ID));
    assert_eq!(plugin.manifest.provides.bridges[0].bridge_id, ABI_BRIDGE_ID);
}
