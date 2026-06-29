use std::collections::{BTreeMap, BTreeSet};

use mutsuki_runtime_contracts::{
    ArtifactType, ContractSurface, ContractSurfaceKind, LifecyclePolicy, PermissionGrant,
    PluginArtifact, PluginDeploymentKind, PluginManifest, PluginProvides, RunnerDescriptor,
    RuntimeCapabilityGraph, RuntimeLoadPlan, RuntimeProfile, RuntimeProfileMode,
};
use mutsuki_runtime_core::RuntimeResult;

use crate::error::{deployment_mismatch, plugin_not_found, required_capability_missing};

#[derive(Default)]
struct ResolvedPlugins {
    manifests: Vec<PluginManifest>,
    deployments: BTreeMap<String, PluginDeploymentKind>,
}

pub fn resolve_load_plan(
    manifests: &[PluginManifest],
    profile: &RuntimeProfile,
) -> RuntimeResult<RuntimeLoadPlan> {
    let resolved = resolve_enabled_plugins(manifests, profile)?;
    let runner_bindings = runner_bindings(profile, &resolved.manifests);
    let capability_graph =
        capability_graph_for(profile, &resolved.manifests, &resolved.deployments)?;
    let contract_surfaces = surfaces_for(&resolved.manifests, &capability_graph);
    Ok(RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: profile.profile_id.clone(),
        profile_hash: profile_hash(profile, resolved.deployments.len(), &capability_graph),
        registry_generation: 1,
        plugins: resolved.manifests,
        load_order: profile.enabled_plugins.clone(),
        runner_bindings,
        plugin_deployments: resolved.deployments,
        capability_graph,
        contract_surfaces,
    })
}

fn resolve_enabled_plugins(
    manifests: &[PluginManifest],
    profile: &RuntimeProfile,
) -> RuntimeResult<ResolvedPlugins> {
    let mut resolved = ResolvedPlugins::default();
    for plugin_id in &profile.enabled_plugins {
        let manifest = manifests
            .iter()
            .find(|manifest| manifest.plugin_id == *plugin_id)
            .ok_or_else(|| plugin_not_found(plugin_id))?;
        let deployment = deployment_for(profile, manifest);
        ensure_deployment_matches_artifact(
            plugin_id,
            &deployment,
            &manifest.artifact.artifact_type,
        )?;
        resolved.deployments.insert(plugin_id.clone(), deployment);
        resolved.manifests.push(manifest.clone());
    }
    Ok(resolved)
}

fn deployment_for(profile: &RuntimeProfile, manifest: &PluginManifest) -> PluginDeploymentKind {
    profile
        .plugin_deployments
        .get(&manifest.plugin_id)
        .cloned()
        .unwrap_or_else(|| {
            PluginDeploymentKind::default_for_artifact(&manifest.artifact.artifact_type)
        })
}

fn ensure_deployment_matches_artifact(
    plugin_id: &str,
    deployment: &PluginDeploymentKind,
    artifact_type: &ArtifactType,
) -> RuntimeResult<()> {
    if deployment.is_compatible_with_artifact(artifact_type) {
        return Ok(());
    }
    Err(deployment_mismatch(
        "host.plugin.deployment_artifact_mismatch",
        plugin_id,
        deployment,
        &PluginDeploymentKind::default_for_artifact(artifact_type),
    ))
}

fn runner_bindings(
    profile: &RuntimeProfile,
    manifests: &[PluginManifest],
) -> BTreeMap<String, String> {
    let mut runner_bindings = profile.bindings.clone();
    for manifest in manifests {
        for runner in &manifest.provides.runners {
            for protocol_id in &runner.accepted_protocol_ids {
                runner_bindings
                    .entry(protocol_id.clone())
                    .or_insert_with(|| runner.runner_id.clone());
            }
        }
    }
    runner_bindings
}

fn profile_hash(
    profile: &RuntimeProfile,
    deployment_count: usize,
    capability_graph: &RuntimeCapabilityGraph,
) -> String {
    format!(
        "profile:{}:{:?}:{}:{}:{}",
        profile.profile_id,
        profile.mode,
        profile.enabled_plugins.len(),
        deployment_count,
        capability_graph.active_capabilities.len()
    )
}

fn capability_graph_for(
    profile: &RuntimeProfile,
    manifests: &[PluginManifest],
    deployments: &BTreeMap<String, PluginDeploymentKind>,
) -> RuntimeResult<RuntimeCapabilityGraph> {
    let prune_extensions = matches!(
        profile.mode,
        RuntimeProfileMode::BuiltinOnly | RuntimeProfileMode::LockedBuiltin
    );
    let mut provided = BTreeSet::new();
    let mut required = BTreeSet::new();
    let mut active = BTreeSet::new();
    let mut active_resource_providers = BTreeSet::new();
    let mut active_host_backends = BTreeSet::new();
    let mut active_plugin_backends = BTreeSet::new();
    let mut active_codecs = BTreeSet::new();
    let mut active_bridges = BTreeSet::new();
    let mut active_scheduler_policies = BTreeSet::new();
    let mut active_workflows = BTreeSet::new();

    for manifest in manifests {
        required.extend(manifest.requires.iter().cloned());
        collect_base_capabilities(manifest, &mut provided, &mut active);
        collect_system_extension_capabilities(manifest, &mut provided);
    }

    for manifest in manifests {
        for backend in &manifest.provides.host_backends {
            if should_activate_extension(
                prune_extensions,
                &required,
                "host_backend",
                &backend.backend_id,
                deployment_is_used(deployments, &backend.supported_deployments),
            ) {
                activate(
                    &mut active,
                    &mut active_host_backends,
                    "host_backend",
                    &backend.backend_id,
                );
            }
        }
        for provider_id in &manifest.provides.resource_providers {
            let provider_is_required = resource_provider_is_used(manifests, provider_id)
                || requires_capability(&required, "resource_strategy", provider_id);
            if should_activate_extension(
                prune_extensions,
                &required,
                "resource_provider",
                provider_id,
                provider_is_required,
            ) {
                activate(
                    &mut active,
                    &mut active_resource_providers,
                    "resource_provider",
                    provider_id,
                );
            }
        }
        for backend in &manifest.provides.plugin_backends {
            if should_activate_extension(
                prune_extensions,
                &required,
                "plugin_backend",
                &backend.backend_id,
                deployment_is_used(deployments, std::slice::from_ref(&backend.deployment_kind)),
            ) {
                activate(
                    &mut active,
                    &mut active_plugin_backends,
                    "plugin_backend",
                    &backend.backend_id,
                );
                if let Some(codec_id) = &backend.codec_id {
                    active_codecs.insert(codec_id.clone());
                }
                if let Some(bridge_id) = &backend.bridge_id {
                    active_bridges.insert(bridge_id.clone());
                }
            }
        }
        for bridge in &manifest.provides.bridges {
            if should_activate_extension(
                prune_extensions,
                &required,
                "bridge",
                &bridge.bridge_id,
                active_bridges.contains(&bridge.bridge_id),
            ) {
                activate(
                    &mut active,
                    &mut active_bridges,
                    "bridge",
                    &bridge.bridge_id,
                );
                active_codecs.extend(bridge.codec_ids.iter().cloned());
            }
        }
        for codec in &manifest.provides.codecs {
            if should_activate_extension(
                prune_extensions,
                &required,
                "codec",
                &codec.codec_id,
                active_codecs.contains(&codec.codec_id),
            ) {
                activate(&mut active, &mut active_codecs, "codec", &codec.codec_id);
            }
        }
        for policy in &manifest.provides.scheduler_policies {
            if should_activate_extension(
                prune_extensions,
                &required,
                "scheduler_policy",
                &policy.policy_id,
                false,
            ) {
                activate(
                    &mut active,
                    &mut active_scheduler_policies,
                    "scheduler_policy",
                    &policy.policy_id,
                );
            }
        }
        for workflow in &manifest.provides.workflows {
            if should_activate_extension(
                prune_extensions,
                &required,
                "workflow",
                &workflow.workflow_id,
                false,
            ) {
                activate(
                    &mut active,
                    &mut active_workflows,
                    "workflow",
                    &workflow.workflow_id,
                );
            }
        }
    }

    ensure_required_capabilities_are_active(&required, &active, &active_resource_providers)?;

    Ok(RuntimeCapabilityGraph {
        profile_mode: profile.mode.clone(),
        provided_capabilities: provided.into_iter().collect(),
        required_capabilities: required.into_iter().collect(),
        active_capabilities: active.into_iter().collect(),
        active_resource_providers: active_resource_providers.into_iter().collect(),
        active_host_backends: active_host_backends.into_iter().collect(),
        active_plugin_backends: active_plugin_backends.into_iter().collect(),
        active_codecs: active_codecs.into_iter().collect(),
        active_bridges: active_bridges.into_iter().collect(),
        active_scheduler_policies: active_scheduler_policies.into_iter().collect(),
        active_workflows: active_workflows.into_iter().collect(),
    })
}

fn collect_base_capabilities(
    manifest: &PluginManifest,
    provided: &mut BTreeSet<String>,
    active: &mut BTreeSet<String>,
) {
    for runner in &manifest.provides.runners {
        insert_capability(provided, "runner", &runner.runner_id);
        insert_capability(active, "runner", &runner.runner_id);
        for protocol_id in &runner.accepted_protocol_ids {
            insert_capability(provided, "task_protocol", protocol_id);
            insert_capability(active, "task_protocol", protocol_id);
        }
    }
    for protocol in &manifest.provides.protocols {
        insert_capability(provided, "protocol", &protocol.protocol_id);
        insert_capability(active, "protocol", &protocol.protocol_id);
    }
    for binding in &manifest.provides.handler_bindings {
        insert_capability(provided, "handler_binding", &binding.binding_id);
        insert_capability(active, "handler_binding", &binding.binding_id);
    }
    for (prefix, names) in [
        ("resource_schema", &manifest.provides.resource_schemas),
        ("effect", &manifest.provides.effects),
        ("stream", &manifest.provides.streams),
        ("subscription", &manifest.provides.subscriptions),
        ("timer", &manifest.provides.timers),
        ("state_schema", &manifest.provides.state_schemas),
    ] {
        for name in names {
            insert_capability(provided, prefix, name);
            insert_capability(active, prefix, name);
        }
    }
    for resource_type in &manifest.provides.resource_types {
        insert_capability(provided, "resource_schema", &resource_type.kind_id);
        insert_capability(provided, "resource_type", &resource_type.kind_id);
        insert_capability(active, "resource_schema", &resource_type.kind_id);
        insert_capability(active, "resource_type", &resource_type.kind_id);
    }
}

fn collect_system_extension_capabilities(
    manifest: &PluginManifest,
    provided: &mut BTreeSet<String>,
) {
    for backend in &manifest.provides.host_backends {
        insert_capability(provided, "host_backend", &backend.backend_id);
    }
    for backend in &manifest.provides.plugin_backends {
        insert_capability(provided, "plugin_backend", &backend.backend_id);
    }
    for provider_id in &manifest.provides.resource_providers {
        insert_capability(provided, "resource_provider", provider_id);
    }
    for codec in &manifest.provides.codecs {
        insert_capability(provided, "codec", &codec.codec_id);
    }
    for bridge in &manifest.provides.bridges {
        insert_capability(provided, "bridge", &bridge.bridge_id);
    }
    for policy in &manifest.provides.scheduler_policies {
        insert_capability(provided, "scheduler_policy", &policy.policy_id);
    }
    for workflow in &manifest.provides.workflows {
        insert_capability(provided, "workflow", &workflow.workflow_id);
    }
}

fn deployment_is_used(
    deployments: &BTreeMap<String, PluginDeploymentKind>,
    candidates: &[PluginDeploymentKind],
) -> bool {
    candidates.iter().any(|candidate| {
        deployments
            .values()
            .any(|deployment| deployment == candidate)
    })
}

fn requires_capability(required: &BTreeSet<String>, prefix: &str, id: &str) -> bool {
    required.contains(id) || required.contains(&format!("{prefix}:{id}"))
}

fn should_activate_extension(
    prune_extensions: bool,
    required: &BTreeSet<String>,
    prefix: &str,
    id: &str,
    used_by_active_plan: bool,
) -> bool {
    !prune_extensions || used_by_active_plan || requires_capability(required, prefix, id)
}

fn resource_provider_is_used(manifests: &[PluginManifest], provider_id: &str) -> bool {
    manifests
        .iter()
        .flat_map(|manifest| manifest.provides.resource_types.iter())
        .any(|resource_type| resource_type.provider_id == provider_id)
}

fn ensure_required_capabilities_are_active(
    required: &BTreeSet<String>,
    active: &BTreeSet<String>,
    active_resource_providers: &BTreeSet<String>,
) -> RuntimeResult<()> {
    for capability in required {
        if active.contains(capability) || unprefixed_capability_is_active(capability, active) {
            continue;
        }
        if let Some(provider_id) = capability.strip_prefix("resource_strategy:")
            && active_resource_providers.contains(provider_id)
        {
            continue;
        }
        return Err(required_capability_missing(capability));
    }
    Ok(())
}

fn unprefixed_capability_is_active(capability: &str, active: &BTreeSet<String>) -> bool {
    if capability.contains(':') {
        return false;
    }
    active.iter().any(|active_capability| {
        active_capability
            .rsplit_once(':')
            .is_some_and(|(_, id)| id == capability)
    })
}

fn activate(
    active: &mut BTreeSet<String>,
    active_ids: &mut BTreeSet<String>,
    prefix: &str,
    id: &str,
) {
    insert_capability(active, prefix, id);
    active_ids.insert(id.into());
}

fn insert_capability(capabilities: &mut BTreeSet<String>, prefix: &str, id: &str) {
    capabilities.insert(format!("{prefix}:{id}"));
}

fn surfaces_for(
    manifests: &[PluginManifest],
    capability_graph: &RuntimeCapabilityGraph,
) -> Vec<ContractSurface> {
    let mut surfaces = Vec::new();
    for manifest in manifests {
        push_runner_surfaces(&mut surfaces, manifest);
        push_protocol_surfaces(&mut surfaces, manifest);
        push_handler_binding_surfaces(&mut surfaces, manifest);
        push_named_capability_surfaces(&mut surfaces, manifest);
        push_resource_provider_surfaces(&mut surfaces, manifest, capability_graph);
        push_resource_type_surfaces(&mut surfaces, manifest);
        push_system_extension_surfaces(&mut surfaces, manifest, capability_graph);
    }
    surfaces
}

fn push_resource_provider_surfaces(
    surfaces: &mut Vec<ContractSurface>,
    manifest: &PluginManifest,
    capability_graph: &RuntimeCapabilityGraph,
) {
    for provider_id in &manifest.provides.resource_providers {
        if !active_surface(&capability_graph.active_resource_providers, provider_id) {
            continue;
        }
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::ResourceProvider,
            format!("resource_provider:{provider_id}"),
            format!("resource_provider:{provider_id}"),
        );
    }
}

fn push_runner_surfaces(surfaces: &mut Vec<ContractSurface>, manifest: &PluginManifest) {
    for runner in &manifest.provides.runners {
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::Runner,
            format!("runner:{}", runner.runner_id),
            format!("runner:{}:{}", runner.runner_id, runner.plugin_generation),
        );
        for protocol_id in &runner.accepted_protocol_ids {
            push_surface(
                surfaces,
                &manifest.plugin_id,
                ContractSurfaceKind::TaskProtocol,
                format!("task_protocol:{protocol_id}"),
                format!("task_protocol:{protocol_id}"),
            );
        }
    }
}

fn push_protocol_surfaces(surfaces: &mut Vec<ContractSurface>, manifest: &PluginManifest) {
    for protocol in &manifest.provides.protocols {
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::Protocol,
            format!("protocol:{}", protocol.protocol_id),
            format!("protocol:{}:{}", protocol.protocol_id, protocol.version),
        );
    }
}

fn push_handler_binding_surfaces(surfaces: &mut Vec<ContractSurface>, manifest: &PluginManifest) {
    for binding in &manifest.provides.handler_bindings {
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::HandlerBinding,
            format!("handler_binding:{}", binding.binding_id),
            format!(
                "handler_binding:{}:{}:{}",
                binding.binding_id, binding.protocol_id, binding.target_protocol_id
            ),
        );
    }
}

fn push_named_capability_surfaces(surfaces: &mut Vec<ContractSurface>, manifest: &PluginManifest) {
    for (kind, prefix, names) in [
        (
            ContractSurfaceKind::ResourceSchema,
            "resource_schema",
            &manifest.provides.resource_schemas,
        ),
        (
            ContractSurfaceKind::Effect,
            "effect",
            &manifest.provides.effects,
        ),
        (
            ContractSurfaceKind::Stream,
            "stream",
            &manifest.provides.streams,
        ),
        (
            ContractSurfaceKind::Subscription,
            "subscription",
            &manifest.provides.subscriptions,
        ),
        (
            ContractSurfaceKind::Timer,
            "timer",
            &manifest.provides.timers,
        ),
        (
            ContractSurfaceKind::StateSchema,
            "state_schema",
            &manifest.provides.state_schemas,
        ),
    ] {
        push_named_surfaces(surfaces, &manifest.plugin_id, kind, prefix, names);
    }
}

fn push_resource_type_surfaces(surfaces: &mut Vec<ContractSurface>, manifest: &PluginManifest) {
    for resource_type in &manifest.provides.resource_types {
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::ResourceSchema,
            format!("resource_schema:{}", resource_type.kind_id),
            format!(
                "resource_type:{}:{:?}:{}:{}:{}:{}:{}",
                resource_type.kind_id,
                resource_type.semantic,
                resource_type.schema,
                resource_type.provider_id,
                resource_type.operations.join(","),
                resource_type.compatibility.schema_version,
                resource_type.compatibility.required_operations.join(",")
            ),
        );
    }
}

fn push_system_extension_surfaces(
    surfaces: &mut Vec<ContractSurface>,
    manifest: &PluginManifest,
    capability_graph: &RuntimeCapabilityGraph,
) {
    for backend in &manifest.provides.host_backends {
        if !active_surface(&capability_graph.active_host_backends, &backend.backend_id) {
            continue;
        }
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::HostBackend,
            format!("host_backend:{}", backend.backend_id),
            format!(
                "host_backend:{}:{:?}:{}:{}",
                backend.backend_id, backend.kind, backend.reload_policy, backend.drain_required
            ),
        );
    }
    for backend in &manifest.provides.plugin_backends {
        if !active_surface(
            &capability_graph.active_plugin_backends,
            &backend.backend_id,
        ) {
            continue;
        }
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::PluginBackend,
            format!("plugin_backend:{}", backend.backend_id),
            format!(
                "plugin_backend:{}:{:?}:{}:{}",
                backend.backend_id,
                backend.deployment_kind,
                backend.task_client_protocol,
                backend.resource_client_protocol
            ),
        );
    }
    for codec in &manifest.provides.codecs {
        if !active_surface(&capability_graph.active_codecs, &codec.codec_id) {
            continue;
        }
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::Codec,
            format!("codec:{}", codec.codec_id),
            format!(
                "codec:{}:{}:{}:{}",
                codec.codec_id, codec.media_type, codec.version, codec.connection_scoped
            ),
        );
    }
    for bridge in &manifest.provides.bridges {
        if !active_surface(&capability_graph.active_bridges, &bridge.bridge_id) {
            continue;
        }
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::Bridge,
            format!("bridge:{}", bridge.bridge_id),
            format!(
                "bridge:{}:{:?}:{}:{}",
                bridge.bridge_id,
                bridge.deployment_kind,
                bridge.codec_ids.join(","),
                bridge.drain_policy
            ),
        );
    }
    for policy in &manifest.provides.scheduler_policies {
        if !active_surface(
            &capability_graph.active_scheduler_policies,
            &policy.policy_id,
        ) {
            continue;
        }
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::SchedulerPolicy,
            format!("scheduler_policy:{}", policy.policy_id),
            format!(
                "scheduler_policy:{}:{}:{}",
                policy.policy_id, policy.version, policy.decision_scope
            ),
        );
    }
    for workflow in &manifest.provides.workflows {
        if !active_surface(&capability_graph.active_workflows, &workflow.workflow_id) {
            continue;
        }
        push_surface(
            surfaces,
            &manifest.plugin_id,
            ContractSurfaceKind::Workflow,
            format!("workflow:{}", workflow.workflow_id),
            format!(
                "workflow:{}:{}:{}:{}",
                workflow.workflow_id,
                workflow.state_resource_kind,
                workflow.runner_protocol_id,
                workflow.reload_policy
            ),
        );
    }
}

fn active_surface(active_ids: &[String], id: &str) -> bool {
    active_ids.iter().any(|active_id| active_id == id)
}

fn push_named_surfaces(
    surfaces: &mut Vec<ContractSurface>,
    plugin_id: &str,
    kind: ContractSurfaceKind,
    prefix: &str,
    names: &[String],
) {
    for name in names {
        push_surface(
            surfaces,
            plugin_id,
            kind.clone(),
            format!("{prefix}:{name}"),
            format!("{prefix}:{name}"),
        );
    }
}

fn push_surface(
    surfaces: &mut Vec<ContractSurface>,
    owner_plugin_id: &str,
    kind: ContractSurfaceKind,
    surface_id: String,
    fingerprint: String,
) {
    surfaces.push(ContractSurface {
        surface_id,
        kind,
        owner_plugin_id: owner_plugin_id.into(),
        fingerprint,
        deprecated: false,
    });
}

pub(crate) fn core_manifest(runner: RunnerDescriptor) -> PluginManifest {
    PluginManifest {
        plugin_id: "core".into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "core".into(),
            sha256: "sha256:core".into(),
        },
        provides: PluginProvides {
            runners: vec![runner],
            ..PluginProvides::default()
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: Vec::new(),
            resources: Vec::new(),
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "core".into(),
            unload_timeout_ms: 0,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: true,
        },
        metadata: BTreeMap::new(),
    }
}
