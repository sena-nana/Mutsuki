use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ArtifactType, ContractSurface, ContractSurfaceKind, LifecyclePolicy, PermissionGrant,
    PluginArtifact, PluginDeploymentKind, PluginManifest, PluginProvides, RunnerDescriptor,
    RuntimeLoadPlan, RuntimeProfile,
};
use mutsuki_runtime_core::RuntimeResult;

use crate::error::{deployment_mismatch, plugin_not_found};

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
    let contract_surfaces = surfaces_for(&resolved.manifests);
    Ok(RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: profile.profile_id.clone(),
        profile_hash: profile_hash(profile, resolved.deployments.len()),
        registry_generation: 1,
        plugins: resolved.manifests,
        load_order: profile.enabled_plugins.clone(),
        runner_bindings,
        plugin_deployments: resolved.deployments,
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

fn profile_hash(profile: &RuntimeProfile, deployment_count: usize) -> String {
    format!(
        "profile:{}:{}:{}",
        profile.profile_id,
        profile.enabled_plugins.len(),
        deployment_count
    )
}

fn surfaces_for(manifests: &[PluginManifest]) -> Vec<ContractSurface> {
    let mut surfaces = Vec::new();
    for manifest in manifests {
        push_runner_surfaces(&mut surfaces, manifest);
        push_protocol_surfaces(&mut surfaces, manifest);
        push_handler_binding_surfaces(&mut surfaces, manifest);
        push_named_capability_surfaces(&mut surfaces, manifest);
        push_resource_type_surfaces(&mut surfaces, manifest);
        push_system_extension_surfaces(&mut surfaces, manifest);
    }
    surfaces
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
            ContractSurfaceKind::ResourceProvider,
            "resource_provider",
            &manifest.provides.resource_providers,
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

fn push_system_extension_surfaces(surfaces: &mut Vec<ContractSurface>, manifest: &PluginManifest) {
    for backend in &manifest.provides.host_backends {
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
