use std::collections::BTreeMap;

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::{runner_manifest, runner_manifest_with_artifact};

use super::helpers::{descriptor, runtime_profile};

#[test]
fn resolver_records_builtin_and_abi_plugin_deployments() {
    let builtin_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut abi_descriptor = descriptor("abi.runner", "abi.work");
    abi_descriptor.plugin_id = "plugin-b".into();
    let builtin_manifest = runner_manifest("plugin-a", vec![builtin_descriptor]);
    let abi_manifest = runner_manifest_with_artifact(
        "plugin-b",
        PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: "plugin-b.abi".into(),
            sha256: "sha256:abi".into(),
        },
        vec![abi_descriptor],
    );
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec!["plugin-a".into(), "plugin-b".into()];
    profile
        .plugin_deployments
        .insert("plugin-a".into(), PluginDeploymentKind::Builtin);
    profile
        .plugin_deployments
        .insert("plugin-b".into(), PluginDeploymentKind::Abi);

    let plan = crate::resolve_load_plan(&[builtin_manifest, abi_manifest], &profile).unwrap();

    assert_eq!(
        plan.plugin_deployments.get("plugin-a"),
        Some(&PluginDeploymentKind::Builtin)
    );
    assert_eq!(
        plan.plugin_deployments.get("plugin-b"),
        Some(&PluginDeploymentKind::Abi)
    );
}

#[test]
fn resolver_emits_declared_runtime_surfaces() {
    let runner_descriptor = descriptor("echo.runner", "raw.input");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.provides.protocols = vec![ProtocolDescriptor {
        protocol_id: "im.message.received.v1".into(),
        version: "1.0.0".into(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        error_schema: json!({"type": "object"}),
        codec: "json".into(),
        compatibility: "semver".into(),
    }];
    manifest.provides.handler_bindings = vec![HandlerBinding {
        binding_id: "message-handler".into(),
        plugin_id: "plugin-a".into(),
        protocol_id: "im.message.received.v1".into(),
        target_protocol_id: "raw.input".into(),
        target_runner_hint: Some("echo.runner".into()),
        pool_id: "default".into(),
        priority: 1,
        policy: "required".into(),
        metadata: BTreeMap::new(),
    }];
    manifest.provides.resource_schemas = vec!["bytes.v1".into()];
    manifest.provides.resource_providers = vec!["resource.local".into()];
    manifest.provides.resource_types = vec![ResourceTypeDescriptor {
        kind_id: "bytes".into(),
        semantic: ResourceSemantic::FrozenValue,
        schema: "bytes.v1".into(),
        provider_id: "resource.local".into(),
        operations: vec!["read".into(), "export".into()],
        reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
        compatibility: ResourceProviderCompatibility {
            schema_version: "1.0.0".into(),
            required_operations: vec!["read".into(), "export".into()],
            preserves_resource_type_id: true,
            accepts_older_generations: true,
            lease_drain_required: true,
        },
    }];
    manifest.provides.effects = vec!["effect.chat.send".into()];
    manifest.provides.streams = vec!["chat.events".into()];
    manifest.provides.subscriptions = vec!["chat.messages".into()];
    manifest.provides.timers = vec!["heartbeat".into()];
    manifest.provides.state_schemas = vec!["state.actor.v1".into()];
    manifest.provides.host_backends = vec![HostBackendDescriptor {
        backend_id: "host.backend.builtin".into(),
        kind: HostExtensionKind::PluginBackend,
        supported_deployments: vec![PluginDeploymentKind::Builtin],
        reload_policy: "drain_and_swap".into(),
        drain_required: true,
    }];
    manifest.provides.plugin_backends = vec![PluginBackendDescriptor {
        backend_id: "plugin.backend.builtin".into(),
        deployment_kind: PluginDeploymentKind::Builtin,
        task_client_protocol: "mutsuki.task.v1".into(),
        resource_client_protocol: "mutsuki.resource-plan.v1".into(),
        codec_id: Some("codec.json".into()),
        bridge_id: None,
    }];
    manifest.provides.codecs = vec![CodecDescriptor {
        codec_id: "codec.json".into(),
        media_type: "application/json".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    }];
    manifest.provides.bridges = vec![BridgeDescriptor {
        bridge_id: "bridge.abi.jsonl".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        codec_ids: vec!["codec.json".into()],
        drain_policy: "connection_drain".into(),
    }];
    manifest.provides.scheduler_policies = vec![SchedulerPolicyDescriptor {
        policy_id: "scheduler.fair".into(),
        version: "1.0.0".into(),
        decision_scope: "dispatch_budget".into(),
    }];
    manifest.provides.workflows = vec![WorkflowDescriptor {
        workflow_id: "workflow.linear".into(),
        state_resource_kind: "workflow.instance".into(),
        runner_protocol_id: "workflow.linear.run".into(),
        reload_policy: "state_resource_handoff".into(),
    }];
    let profile = RuntimeProfile {
        profile_id: "default".into(),
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    };

    let plan = crate::resolve_load_plan(&[manifest], &profile).unwrap();

    assert_surface(
        &plan,
        "protocol:im.message.received.v1",
        ContractSurfaceKind::Protocol,
    );
    assert_surface(
        &plan,
        "handler_binding:message-handler",
        ContractSurfaceKind::HandlerBinding,
    );
    assert_surface(
        &plan,
        "resource_schema:bytes.v1",
        ContractSurfaceKind::ResourceSchema,
    );
    assert_surface(
        &plan,
        "resource_provider:resource.local",
        ContractSurfaceKind::ResourceProvider,
    );
    assert_surface(
        &plan,
        "effect:effect.chat.send",
        ContractSurfaceKind::Effect,
    );
    assert_surface(&plan, "stream:chat.events", ContractSurfaceKind::Stream);
    assert_surface(
        &plan,
        "subscription:chat.messages",
        ContractSurfaceKind::Subscription,
    );
    assert_surface(&plan, "timer:heartbeat", ContractSurfaceKind::Timer);
    assert_surface(
        &plan,
        "state_schema:state.actor.v1",
        ContractSurfaceKind::StateSchema,
    );
    assert_surface(
        &plan,
        "host_backend:host.backend.builtin",
        ContractSurfaceKind::HostBackend,
    );
    assert_surface(
        &plan,
        "plugin_backend:plugin.backend.builtin",
        ContractSurfaceKind::PluginBackend,
    );
    assert_surface(&plan, "codec:codec.json", ContractSurfaceKind::Codec);
    assert_surface(
        &plan,
        "bridge:bridge.abi.jsonl",
        ContractSurfaceKind::Bridge,
    );
    assert_surface(
        &plan,
        "scheduler_policy:scheduler.fair",
        ContractSurfaceKind::SchedulerPolicy,
    );
    assert_surface(
        &plan,
        "workflow:workflow.linear",
        ContractSurfaceKind::Workflow,
    );
}

fn assert_surface(plan: &RuntimeLoadPlan, surface_id: &str, kind: ContractSurfaceKind) {
    assert!(
        plan.contract_surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.kind == kind),
        "missing surface {surface_id}"
    );
}
