use std::collections::BTreeMap;

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::{runner_manifest, runner_manifest_with_artifact};

use super::helpers::{descriptor, runtime_profile};

#[test]
fn resolver_activates_owner_defined_capabilities_and_fences_consumers() {
    let mut provider = runner_manifest("agent-connections", Vec::new());
    provider.provides.capabilities = vec!["agent_connection:primary".into()];
    let mut consumer = runner_manifest("bot-agent", Vec::new());
    consumer.requires = vec![SurfaceRequirement::new(
        ContractSurfaceKind::Capability,
        "agent_connection:primary",
    )];
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec!["agent-connections".into(), "bot-agent".into()];

    let plan = crate::resolve_load_plan(&[provider, consumer.clone()], &profile).unwrap();
    assert!(
        plan.capability_graph
            .active_capabilities
            .contains(&"capability:agent_connection:primary".into())
    );
    assert_eq!(
        plan.capability_graph
            .active_capability_providers
            .iter()
            .find(|selection| selection.capability == "capability:agent_connection:primary")
            .map(|selection| selection.provider_plugin_id.as_str()),
        Some("agent-connections")
    );

    profile.enabled_plugins = vec!["bot-agent".into()];
    let error = crate::resolve_load_plan(&[consumer], &profile).unwrap_err();
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn resolver_normalizes_legacy_protocol_classes_and_accepted_protocols() {
    let mut runner = descriptor("legacy.runner", "sim.beta");
    runner.accepted_protocol_ids = vec!["sim.beta".into(), "sim.alpha".into(), "sim.beta".into()];
    let manifest = runner_manifest("plugin-a", vec![runner]);

    let plan = crate::resolve_load_plan(&[manifest], &runtime_profile()).unwrap();
    let provides = &plan.plugins[0].provides;

    assert_eq!(
        provides.runners[0].accepted_protocol_ids,
        vec!["sim.alpha", "sim.beta"]
    );
    assert_eq!(
        provides.protocol_classes.get("sim.alpha"),
        Some(&ProtocolClass::Domain)
    );
    assert_eq!(
        provides.protocol_classes.get("sim.beta"),
        Some(&ProtocolClass::Domain)
    );
}

#[test]
fn resolver_rejects_protocol_class_purity_conflicts() {
    let runner = descriptor("conflict.runner", "sim.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner]);
    manifest
        .provides
        .protocol_classes
        .insert("sim.work".into(), ProtocolClass::Effect);

    let error = crate::resolve_load_plan(&[manifest], &runtime_profile()).unwrap_err();

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert!(error.error().route.contains("purity_conflict"));
}

#[test]
fn resolver_rejects_protocol_classes_for_unknown_protocols() {
    let runner = descriptor("unknown.runner", "sim.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner]);
    manifest
        .provides
        .protocol_classes
        .insert("sim.unknown".into(), ProtocolClass::Domain);

    let error = crate::resolve_load_plan(&[manifest], &runtime_profile()).unwrap_err();

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert!(error.error().route.contains("unknown_protocol"));
}

#[test]
fn resolver_rejects_cross_plugin_protocol_class_conflicts() {
    let protocol = |plugin_id: &str, class| {
        let mut manifest = runner_manifest(plugin_id, Vec::new());
        manifest.provides.protocols = vec![ProtocolDescriptor {
            protocol_id: "shared.protocol".into(),
            version: "1.0.0".into(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            error_schema: json!({"type": "object"}),
            codec: "json".into(),
            compatibility: "semver".into(),
        }];
        manifest
            .provides
            .protocol_classes
            .insert("shared.protocol".into(), class);
        manifest
    };
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec!["plugin-a".into(), "plugin-b".into()];

    let error = crate::resolve_load_plan(
        &[
            protocol("plugin-a", ProtocolClass::Domain),
            protocol("plugin-b", ProtocolClass::Effect),
        ],
        &profile,
    )
    .unwrap_err();

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert!(error.error().route.contains("cross_plugin_conflict"));
}

#[test]
fn resolver_rejects_empty_production_protocol_schema() {
    let mut manifest =
        runner_manifest("plugin-a", vec![descriptor("schema.runner", "schema.work")]);
    manifest.provides.protocols = vec![ProtocolDescriptor {
        protocol_id: "schema.work".into(),
        version: "1.0.0".into(),
        input_schema: json!({}),
        output_schema: json!({"type": "object"}),
        error_schema: json!({"type": "object"}),
        codec: "json".into(),
        compatibility: "semver".into(),
    }];

    let error = crate::resolve_load_plan(&[manifest], &runtime_profile())
        .expect_err("empty request schema must be rejected before registry freeze");
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(error.error().source, "host.load_plan.protocol_schema");
}

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
            companion_artifacts: Vec::new(),
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
    manifest.provides.resource_providers = vec!["mutsuki.std.resource.memory".into()];
    manifest.provides.resource_types = vec![ResourceTypeDescriptor {
        kind_id: "bytes".into(),
        semantic: ResourceSemantic::FrozenValue,
        schema: "bytes.v1".into(),
        provider_id: "mutsuki.std.resource.memory".into(),
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
    manifest.provides.host_extensions = vec![HostExtensionDescriptor {
        extension_id: "host.extension.builtin".into(),
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
        codec_id: Some("mutsuki.codec.typed-msgpack.v1".into()),
        bridge_id: None,
    }];
    manifest.provides.codecs = vec![CodecDescriptor {
        codec_id: "mutsuki.codec.typed-msgpack.v1".into(),
        media_type: "application/vnd.mutsuki.msgpack".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    }];
    manifest.provides.bridges = vec![BridgeDescriptor {
        bridge_id: "bridge.abi.binary".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        codec_ids: vec!["mutsuki.codec.typed-msgpack.v1".into()],
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
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        surface_bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        observability: ObservabilityProfile::default(),
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
        "resource_provider:mutsuki.std.resource.memory",
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
        "host_extension:host.extension.builtin",
        ContractSurfaceKind::HostExtension,
    );
    assert_surface(
        &plan,
        "plugin_backend:plugin.backend.builtin",
        ContractSurfaceKind::PluginBackend,
    );
    assert_surface(
        &plan,
        "codec:mutsuki.codec.typed-msgpack.v1",
        ContractSurfaceKind::Codec,
    );
    assert_surface(
        &plan,
        "bridge:bridge.abi.binary",
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

#[test]
fn locked_builtin_profile_prunes_unused_external_extensions() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.requires = vec![SurfaceRequirement::new(
        ContractSurfaceKind::Workflow,
        "workflow.linear",
    )];
    manifest.provides.resource_providers = vec![
        "mutsuki.std.resource.memory".into(),
        "resource.shared-memory".into(),
    ];
    manifest.provides.resource_types = vec![frozen_bytes_resource_type(
        "mutsuki.std.resource.memory",
        &["read", "export"],
    )];
    manifest.provides.host_extensions = vec![
        HostExtensionDescriptor {
            extension_id: "host.extension.builtin".into(),
            kind: HostExtensionKind::PluginBackend,
            supported_deployments: vec![PluginDeploymentKind::Builtin],
            reload_policy: "static".into(),
            drain_required: false,
        },
        HostExtensionDescriptor {
            extension_id: "host.extension.abi".into(),
            kind: HostExtensionKind::Bridge,
            supported_deployments: vec![PluginDeploymentKind::Abi],
            reload_policy: "drain_and_swap".into(),
            drain_required: true,
        },
    ];
    manifest.provides.plugin_backends = vec![
        PluginBackendDescriptor {
            backend_id: "plugin.backend.builtin".into(),
            deployment_kind: PluginDeploymentKind::Builtin,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: None,
            bridge_id: None,
        },
        PluginBackendDescriptor {
            backend_id: "plugin.backend.abi".into(),
            deployment_kind: PluginDeploymentKind::Abi,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: Some("mutsuki.codec.typed-msgpack.v1".into()),
            bridge_id: Some("bridge.abi.binary".into()),
        },
    ];
    manifest.provides.codecs = vec![CodecDescriptor {
        codec_id: "mutsuki.codec.typed-msgpack.v1".into(),
        media_type: "application/vnd.mutsuki.msgpack".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    }];
    manifest.provides.bridges = vec![BridgeDescriptor {
        bridge_id: "bridge.abi.binary".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        codec_ids: vec!["mutsuki.codec.typed-msgpack.v1".into()],
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
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::LockedBuiltin;
    profile.allow_dynamic_registration = false;
    profile.allow_hot_reload = false;

    let plan = crate::resolve_load_plan(&[manifest], &profile).unwrap();

    assert_eq!(
        plan.capability_graph.profile_mode,
        RuntimeProfileMode::LockedBuiltin
    );
    assert_eq!(
        plan.capability_graph.active_host_extensions,
        vec!["host.extension.builtin".to_string()]
    );
    assert_eq!(
        plan.capability_graph.active_plugin_backends,
        vec!["plugin.backend.builtin".to_string()]
    );
    assert_eq!(
        plan.capability_graph.active_resource_providers,
        vec!["mutsuki.std.resource.memory".to_string()]
    );
    assert_eq!(
        plan.capability_graph.active_workflows,
        vec!["workflow.linear".to_string()]
    );
    assert!(plan.capability_graph.active_bridges.is_empty());
    assert!(plan.capability_graph.active_codecs.is_empty());
    assert!(plan.capability_graph.active_scheduler_policies.is_empty());
    assert_surface(
        &plan,
        "host_extension:host.extension.builtin",
        ContractSurfaceKind::HostExtension,
    );
    assert_surface(
        &plan,
        "plugin_backend:plugin.backend.builtin",
        ContractSurfaceKind::PluginBackend,
    );
    assert_surface(
        &plan,
        "workflow:workflow.linear",
        ContractSurfaceKind::Workflow,
    );
    assert_surface(
        &plan,
        "resource_provider:mutsuki.std.resource.memory",
        ContractSurfaceKind::ResourceProvider,
    );
    assert_no_surface(&plan, "resource_provider:resource.shared-memory");
    assert_no_surface(&plan, "bridge:bridge.abi.binary");
    assert_no_surface(&plan, "codec:mutsuki.codec.typed-msgpack.v1");
    assert_no_surface(&plan, "scheduler_policy:scheduler.fair");
}

#[test]
fn extensible_runtime_profile_keeps_external_extensions_available() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.provides.plugin_backends = vec![PluginBackendDescriptor {
        backend_id: "plugin.backend.abi".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        task_client_protocol: "mutsuki.task.v1".into(),
        resource_client_protocol: "mutsuki.resource-plan.v1".into(),
        codec_id: Some("mutsuki.codec.typed-msgpack.v1".into()),
        bridge_id: Some("bridge.abi.binary".into()),
    }];
    manifest.provides.codecs = vec![CodecDescriptor {
        codec_id: "mutsuki.codec.typed-msgpack.v1".into(),
        media_type: "application/vnd.mutsuki.msgpack".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    }];
    manifest.provides.bridges = vec![BridgeDescriptor {
        bridge_id: "bridge.abi.binary".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        codec_ids: vec!["mutsuki.codec.typed-msgpack.v1".into()],
        drain_policy: "connection_drain".into(),
    }];
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::ExtensibleRuntime;

    let plan = crate::resolve_load_plan(&[manifest], &profile).unwrap();

    assert_eq!(
        plan.capability_graph.profile_mode,
        RuntimeProfileMode::ExtensibleRuntime
    );
    assert_eq!(
        plan.capability_graph.active_plugin_backends,
        vec!["plugin.backend.abi".to_string()]
    );
    assert_eq!(
        plan.capability_graph.active_bridges,
        vec!["bridge.abi.binary".to_string()]
    );
    assert_eq!(
        plan.capability_graph.active_codecs,
        vec!["mutsuki.codec.typed-msgpack.v1".to_string()]
    );
    assert_surface(
        &plan,
        "plugin_backend:plugin.backend.abi",
        ContractSurfaceKind::PluginBackend,
    );
    assert_surface(
        &plan,
        "bridge:bridge.abi.binary",
        ContractSurfaceKind::Bridge,
    );
    assert_surface(
        &plan,
        "codec:mutsuki.codec.typed-msgpack.v1",
        ContractSurfaceKind::Codec,
    );
}

#[test]
fn builtin_only_profile_prunes_unused_resource_providers() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.provides.resource_providers = vec![
        "mutsuki.std.resource.memory".into(),
        "resource.shared-memory".into(),
    ];
    manifest.provides.resource_types = vec![frozen_bytes_resource_type(
        "mutsuki.std.resource.memory",
        &["read"],
    )];
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::BuiltinOnly;

    let plan = crate::resolve_load_plan(&[manifest], &profile).unwrap();

    assert_eq!(
        plan.capability_graph.profile_mode,
        RuntimeProfileMode::BuiltinOnly
    );
    assert_eq!(
        plan.capability_graph.active_resource_providers,
        vec!["mutsuki.std.resource.memory".to_string()]
    );
    assert!(
        plan.capability_graph
            .provided_capabilities
            .contains(&"resource_provider:resource.shared-memory".to_string())
    );
    assert_surface(
        &plan,
        "resource_provider:mutsuki.std.resource.memory",
        ContractSurfaceKind::ResourceProvider,
    );
    assert_no_surface(&plan, "resource_provider:resource.shared-memory");
}

#[test]
fn resolver_rejects_missing_required_capability() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.requires = vec![SurfaceRequirement::new(
        ContractSurfaceKind::Workflow,
        "workflow.missing",
    )];
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::LockedBuiltin;

    let error = crate::resolve_load_plan(&[manifest], &profile)
        .expect_err("missing required capability should fail")
        .error()
        .clone();

    assert_eq!(error.code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.evidence.get("capability"),
        Some(&ScalarValue::String("workflow:workflow.missing".into()))
    );
}

#[test]
fn resolver_preserves_at_version_inside_task_protocol_identity() {
    let protocol_id = "mutsuki.http.request@2";
    let provider = runner_manifest(
        "http-provider",
        vec![descriptor("http.runner", protocol_id)],
    );
    let mut consumer = runner_manifest(
        "http-consumer",
        vec![descriptor("consumer.runner", "consumer.work")],
    );
    consumer.requires = vec![SurfaceRequirement::task_protocol(protocol_id)];
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec!["http-provider".into(), "http-consumer".into()];

    let plan = crate::resolve_load_plan(&[provider, consumer], &profile).unwrap();

    assert!(
        plan.capability_graph
            .active_capabilities
            .contains(&format!("task_protocol:{protocol_id}"))
    );
    assert!(
        plan.capability_graph
            .required_capabilities
            .contains(&format!("task_protocol:{protocol_id}"))
    );
}

#[test]
fn resolver_requires_explicit_surface_provider_when_multiple_plugins_match() {
    let protocol_id = "shared.work@1";
    let provider_a = runner_manifest(
        "provider-a",
        vec![descriptor("provider-a.runner", protocol_id)],
    );
    let provider_b = runner_manifest(
        "provider-b",
        vec![descriptor("provider-b.runner", protocol_id)],
    );
    let mut consumer = runner_manifest(
        "consumer",
        vec![descriptor("consumer.runner", "consumer.work")],
    );
    consumer.requires = vec![SurfaceRequirement::task_protocol(protocol_id)];
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec!["provider-a".into(), "provider-b".into(), "consumer".into()];
    profile
        .bindings
        .insert(protocol_id.into(), "provider-a.runner".into());

    let error = crate::resolve_load_plan(
        &[provider_a.clone(), provider_b.clone(), consumer.clone()],
        &profile,
    )
    .expect_err("ambiguous surface must not depend on manifest order");
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);

    profile
        .surface_bindings
        .insert(format!("task_protocol:{protocol_id}"), "provider-a".into());
    let plan = crate::resolve_load_plan(&[provider_b, consumer, provider_a], &profile).unwrap();
    assert!(
        plan.capability_graph
            .active_capability_providers
            .iter()
            .any(
                |selection| selection.capability == format!("task_protocol:{protocol_id}")
                    && selection.provider_plugin_id == "provider-a"
            )
    );
}

#[test]
fn resolver_records_capability_provider_selection_and_permission_audit() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.requires = vec![
        SurfaceRequirement::new(ContractSurfaceKind::Protocol, "contract.v1")
            .with_version(">=1.0.0"),
        SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            "mutsuki.std.resource.memory",
        ),
    ];
    manifest.provides.protocols = vec![ProtocolDescriptor {
        protocol_id: "contract.v1".into(),
        version: "1.2.0".into(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        error_schema: json!({"type": "object"}),
        codec: "json".into(),
        compatibility: "semver".into(),
    }];
    manifest.provides.resource_providers = vec!["mutsuki.std.resource.memory".into()];
    manifest.provides.resource_types = vec![frozen_bytes_resource_type(
        "mutsuki.std.resource.memory",
        &["read", "export"],
    )];
    manifest.provides.effects = vec!["effect.chat.send".into()];
    manifest.permissions = PermissionGrant {
        effects: vec!["effect.chat.send".into()],
        resources: vec!["resource:bytes:read".into()],
    };

    let plan = crate::resolve_load_plan(&[manifest], &runtime_profile()).unwrap();

    assert!(
        plan.capability_graph
            .active_capability_providers
            .iter()
            .any(|selection| selection.capability == "protocol:contract.v1"
                && selection.provider_plugin_id == "plugin-a"
                && selection.provider_version.as_deref() == Some("1.2.0")
                && selection.reason == "required_version")
    );
    assert!(
        plan.capability_graph
            .permission_audit
            .iter()
            .any(|entry| entry.plugin_id == "plugin-a"
                && entry.permission_kind == "effect"
                && entry.permission == "effect.chat.send"
                && entry.granted
                && entry.provider_capability.as_deref() == Some("effect:effect.chat.send"))
    );
    assert!(
        plan.capability_graph
            .permission_audit
            .iter()
            .any(|entry| entry.plugin_id == "plugin-a"
                && entry.permission_kind == "resource"
                && entry.permission == "resource:bytes:read"
                && entry.granted
                && entry.provider_capability.as_deref() == Some("resource_type:bytes"))
    );
}

#[test]
fn resolver_uses_protocol_class_for_non_prefixed_effect_permissions_and_surfaces() {
    let mut runner = descriptor("custom.effect.runner", "chat.send.v1");
    runner.purity = RunnerPurity::Effectful;
    let mut manifest = runner_manifest("plugin-a", vec![runner]);
    manifest
        .provides
        .protocol_classes
        .insert("chat.send.v1".into(), ProtocolClass::Effect);
    manifest.permissions.effects = vec!["chat.send.v1".into()];

    let plan = crate::resolve_load_plan(&[manifest], &runtime_profile()).unwrap();

    assert!(
        plan.capability_graph
            .active_capabilities
            .contains(&"effect:chat.send.v1".into())
    );
    assert!(plan.contract_surfaces.iter().any(|surface| {
        surface.surface_id == "effect:chat.send.v1" && surface.kind == ContractSurfaceKind::Effect
    }));
    assert!(plan.capability_graph.permission_audit.iter().any(|entry| {
        entry.permission == "chat.send.v1"
            && entry.provider_capability.as_deref() == Some("effect:chat.send.v1")
    }));
}

#[test]
fn resolver_rejects_unsatisfied_capability_version_constraint() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.requires = vec![
        SurfaceRequirement::new(ContractSurfaceKind::Protocol, "contract.v1")
            .with_version(">=2.0.0"),
    ];
    manifest.provides.protocols = vec![ProtocolDescriptor {
        protocol_id: "contract.v1".into(),
        version: "1.2.0".into(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        error_schema: json!({"type": "object"}),
        codec: "json".into(),
        compatibility: "semver".into(),
    }];

    let error = crate::resolve_load_plan(&[manifest], &runtime_profile())
        .expect_err("unsatisfied capability version should fail")
        .error()
        .clone();

    assert_eq!(error.code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.evidence.get("capability"),
        Some(&ScalarValue::String("protocol:contract.v1".into()))
    );
    assert_eq!(
        error.evidence.get("version_constraint"),
        Some(&ScalarValue::String(">=2.0.0".into()))
    );
}

#[test]
fn resolver_rejects_unbacked_permission_grant() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.permissions = PermissionGrant {
        effects: vec!["effect.chat.send".into()],
        resources: Vec::new(),
    };

    let error = crate::resolve_load_plan(&[manifest], &runtime_profile())
        .expect_err("permission without active provider should fail")
        .error()
        .clone();

    assert_eq!(error.code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.evidence.get("permission_kind"),
        Some(&ScalarValue::String("effect".into()))
    );
    assert_eq!(
        error.evidence.get("permission"),
        Some(&ScalarValue::String("effect.chat.send".into()))
    );
}

#[test]
fn resolver_rejects_plugin_backend_with_missing_codec_provider() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.provides.plugin_backends = vec![PluginBackendDescriptor {
        backend_id: "plugin.backend.builtin".into(),
        deployment_kind: PluginDeploymentKind::Builtin,
        task_client_protocol: "mutsuki.task.v1".into(),
        resource_client_protocol: "mutsuki.resource-plan.v1".into(),
        codec_id: Some("codec.missing".into()),
        bridge_id: None,
    }];

    let error = crate::resolve_load_plan(&[manifest], &runtime_profile())
        .expect_err("active backend codec must have a provider descriptor")
        .error()
        .clone();

    assert_eq!(error.code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.evidence.get("capability"),
        Some(&ScalarValue::String("codec:codec.missing".into()))
    );
}

#[test]
fn resolver_rejects_profile_binding_to_missing_runner() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    let mut profile = runtime_profile();
    profile
        .bindings
        .insert("builtin.work".into(), "missing.runner".into());

    let error = crate::resolve_load_plan(&[manifest], &profile)
        .expect_err("profile binding should point at an enabled runner")
        .error()
        .clone();

    assert_eq!(error.code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.evidence.get("runner_id"),
        Some(&ScalarValue::String("missing.runner".into()))
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

fn assert_no_surface(plan: &RuntimeLoadPlan, surface_id: &str) {
    assert!(
        plan.contract_surfaces
            .iter()
            .all(|surface| surface.surface_id != surface_id),
        "unexpected surface {surface_id}"
    );
}

fn frozen_bytes_resource_type(provider_id: &str, operations: &[&str]) -> ResourceTypeDescriptor {
    let operations: Vec<String> = operations
        .iter()
        .map(|operation| (*operation).into())
        .collect();
    ResourceTypeDescriptor {
        kind_id: "bytes".into(),
        semantic: ResourceSemantic::FrozenValue,
        schema: "bytes.v1".into(),
        provider_id: provider_id.into(),
        operations: operations.clone(),
        reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
        compatibility: ResourceProviderCompatibility {
            schema_version: "1.0.0".into(),
            required_operations: operations,
            preserves_resource_type_id: true,
            accepts_older_generations: true,
            lease_drain_required: true,
        },
    }
}
