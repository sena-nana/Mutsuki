use std::collections::BTreeMap;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::RunnerContext;
use serde_json::json;

use crate::{NativePluginHost, NativeRunner, runner_manifest, runner_manifest_with_artifact};

pub(super) fn descriptor(id: &str, protocol_id: &str) -> RunnerDescriptor {
    descriptor_with_class(id, protocol_id, ExecutionClass::Cpu)
}

pub(super) fn descriptor_with_class(
    id: &str,
    protocol_id: &str,
    execution_class: ExecutionClass,
) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: id.into(),
        plugin_id: "plugin-a".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![protocol_id.into()],
        purity: RunnerPurity::Pure,
        execution_class,
        input_schema: json!({}),
        output_schema: json!({}),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{id}")],
    }
}

pub(super) fn runtime_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "default".into(),
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}

pub(super) fn runtime_profile_with_deployment(
    plugin_id: &str,
    deployment: PluginDeploymentKind,
) -> RuntimeProfile {
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec![plugin_id.into()];
    profile
        .plugin_deployments
        .insert(plugin_id.into(), deployment);
    profile
}

pub(super) fn abi_plugin_fixture() -> (PluginManifest, RunnerDescriptor) {
    let mut runner_descriptor = descriptor("abi.runner", "abi.work");
    runner_descriptor.plugin_id = "plugin-abi".into();
    let manifest = runner_manifest_with_artifact(
        "plugin-abi",
        PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: "plugin-abi.so".into(),
            sha256: "sha256:abi".into(),
        },
        vec![runner_descriptor.clone()],
    );
    (manifest, runner_descriptor)
}

pub(super) fn host_with_echo_runner() -> NativePluginHost {
    let runner_descriptor = descriptor("echo.runner", "raw.input");
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx: RunnerContext, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    host
}

pub(super) fn test_resource_ref(
    ref_id: &str,
    kind_id: &str,
    semantic: ResourceSemantic,
) -> ResourceRef {
    ResourceRef {
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version: 1,
        },
        ref_id: ref_id.into(),
        semantic,
        provider_id: "mutsuki.host.test".into(),
        resource_kind: kind_id.into(),
        schema: format!("{kind_id}.v1"),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: None,
        content_hash: None,
        lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}
