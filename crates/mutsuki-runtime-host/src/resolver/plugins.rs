use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ArtifactType, LifecyclePolicy, PermissionGrant, PluginArtifact, PluginDeploymentKind,
    PluginManifest, PluginProvides, RunnerDescriptor, RuntimeCapabilityGraph, RuntimeProfile,
};
use mutsuki_runtime_core::RuntimeResult;

use crate::error::{deployment_mismatch, plugin_not_found, runner_binding_invalid};

#[derive(Default)]
pub(super) struct ResolvedPlugins {
    pub(super) manifests: Vec<PluginManifest>,
    pub(super) deployments: BTreeMap<String, PluginDeploymentKind>,
}

pub(super) fn resolve_enabled_plugins(
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

pub(super) fn runner_bindings(
    profile: &RuntimeProfile,
    manifests: &[PluginManifest],
) -> RuntimeResult<BTreeMap<String, String>> {
    let mut runner_bindings = profile.bindings.clone();
    validate_profile_bindings(&runner_bindings, manifests)?;
    for manifest in manifests {
        for runner in &manifest.provides.runners {
            for protocol_id in &runner.accepted_protocol_ids {
                runner_bindings
                    .entry(protocol_id.clone())
                    .or_insert_with(|| runner.runner_id.clone());
            }
        }
    }
    Ok(runner_bindings)
}

fn validate_profile_bindings(
    runner_bindings: &BTreeMap<String, String>,
    manifests: &[PluginManifest],
) -> RuntimeResult<()> {
    for (protocol_id, runner_id) in runner_bindings {
        let Some(runner) = manifests
            .iter()
            .flat_map(|manifest| manifest.provides.runners.iter())
            .find(|runner| runner.runner_id == *runner_id)
        else {
            return Err(runner_binding_invalid(
                protocol_id,
                runner_id,
                "runner_not_enabled",
            ));
        };
        if !runner
            .accepted_protocol_ids
            .iter()
            .any(|accepted| accepted == protocol_id)
        {
            return Err(runner_binding_invalid(
                protocol_id,
                runner_id,
                "protocol_not_accepted",
            ));
        }
    }
    Ok(())
}

pub(super) fn profile_hash(
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
