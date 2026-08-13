use std::collections::{BTreeMap, BTreeSet};

use mutsuki_runtime_contracts::{
    ArtifactType, ExecutionClass, LifecyclePolicy, PermissionGrant, PluginArtifact,
    PluginDeploymentKind, PluginManifest, PluginProvides, ProtocolClass, RunnerDescriptor,
    RunnerPurity, RuntimeCapabilityGraph, RuntimeProfile,
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
        let mut manifest = manifest.clone();
        validate_plugin_extensions(&manifest)?;
        validate_protocol_schemas(&manifest)?;
        normalize_protocol_classes(&mut manifest)?;
        resolved.manifests.push(manifest);
    }
    reject_cross_plugin_protocol_class_conflicts(&resolved.manifests)?;
    Ok(resolved)
}

fn validate_protocol_schemas(manifest: &PluginManifest) -> RuntimeResult<()> {
    for protocol in &manifest.provides.protocols {
        for (direction, schema) in [
            ("input", &protocol.input_schema),
            ("output", &protocol.output_schema),
            ("error", &protocol.error_schema),
        ] {
            if schema.is_null() || schema.as_object().is_some_and(serde_json::Map::is_empty) {
                return Err(mutsuki_runtime_core::RuntimeFailure::new(
                    mutsuki_runtime_contracts::RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
                        "host.load_plan.protocol_schema",
                        format!(
                            "plugin.{}.protocol.{}.schema.{direction}.empty",
                            manifest.plugin_id, protocol.protocol_id
                        ),
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_plugin_extensions(manifest: &PluginManifest) -> RuntimeResult<()> {
    let mut identities = BTreeSet::new();
    for extension in &manifest.provides.extensions {
        let extension_id = extension.extension_id.trim();
        let valid_id = !extension_id.is_empty()
            && extension_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
        let valid_payload = extension.payload.is_object();
        if !valid_id || extension.version == 0 || !valid_payload {
            return Err(plugin_extension_error(
                manifest,
                extension_id,
                extension.version,
                "invalid_descriptor",
            ));
        }
        if !identities.insert((extension_id, extension.version)) {
            return Err(plugin_extension_error(
                manifest,
                extension_id,
                extension.version,
                "duplicate_descriptor",
            ));
        }
    }
    Ok(())
}

fn plugin_extension_error(
    manifest: &PluginManifest,
    extension_id: &str,
    version: u32,
    reason: &str,
) -> mutsuki_runtime_core::RuntimeFailure {
    mutsuki_runtime_core::RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "host.load_plan.plugin_extension",
        format!(
            "plugin.{}.extension.{extension_id}@{version}.{reason}",
            manifest.plugin_id
        ),
    ))
}

fn reject_cross_plugin_protocol_class_conflicts(manifests: &[PluginManifest]) -> RuntimeResult<()> {
    let mut classes = BTreeMap::new();
    for manifest in manifests {
        for (protocol_id, class) in &manifest.provides.protocol_classes {
            if let Some(existing) = classes.insert(protocol_id.as_str(), class)
                && existing != class
            {
                return Err(protocol_class_error(
                    manifest,
                    protocol_id,
                    "cross_plugin_conflict",
                ));
            }
        }
    }
    Ok(())
}

fn normalize_protocol_classes(manifest: &mut PluginManifest) -> RuntimeResult<()> {
    let mut known_protocols = manifest
        .provides
        .protocols
        .iter()
        .map(|protocol| protocol.protocol_id.clone())
        .collect::<BTreeSet<_>>();
    for runner in &mut manifest.provides.runners {
        runner.accepted_protocol_ids.sort();
        runner.accepted_protocol_ids.dedup();
        known_protocols.extend(runner.accepted_protocol_ids.iter().cloned());
    }
    known_protocols.extend(manifest.provides.effects.iter().cloned());

    if let Some(unknown) = manifest
        .provides
        .protocol_classes
        .keys()
        .find(|protocol_id| !known_protocols.contains(*protocol_id))
    {
        return Err(protocol_class_error(manifest, unknown, "unknown_protocol"));
    }

    for protocol_id in known_protocols {
        let legacy_class = legacy_protocol_class(&protocol_id);
        let class = manifest
            .provides
            .protocol_classes
            .entry(protocol_id.clone())
            .or_insert(legacy_class.clone());
        if (protocol_id.starts_with("effect.") && class != &ProtocolClass::Effect)
            || (protocol_id.starts_with("core.")
                && !matches!(class, ProtocolClass::Core | ProtocolClass::Control))
            || (manifest.provides.effects.contains(&protocol_id) && class != &ProtocolClass::Effect)
        {
            return Err(protocol_class_error(
                manifest,
                &protocol_id,
                "canonical_conflict",
            ));
        }
    }

    for runner in &manifest.provides.runners {
        for protocol_id in &runner.accepted_protocol_ids {
            let class = manifest
                .provides
                .protocol_classes
                .get(protocol_id)
                .expect("known runner protocol was normalized");
            let purity_matches = matches!(
                (&runner.purity, class),
                (RunnerPurity::Pure, ProtocolClass::Domain)
                    | (RunnerPurity::Effectful, ProtocolClass::Effect)
                    | (RunnerPurity::Committer, ProtocolClass::Core)
                    | (RunnerPurity::Committer, ProtocolClass::Control)
            );
            let control_matches = class != &ProtocolClass::Control
                || runner.execution_class == ExecutionClass::Control;
            if !purity_matches || !control_matches {
                return Err(protocol_class_error(
                    manifest,
                    protocol_id,
                    &format!("runner.{}.purity_conflict", runner.runner_id),
                ));
            }
        }
    }
    Ok(())
}

fn legacy_protocol_class(protocol_id: &str) -> ProtocolClass {
    if protocol_id.starts_with("effect.") {
        ProtocolClass::Effect
    } else if protocol_id.starts_with("core.") {
        ProtocolClass::Core
    } else {
        ProtocolClass::Domain
    }
}

fn protocol_class_error(
    manifest: &PluginManifest,
    protocol_id: &str,
    reason: &str,
) -> mutsuki_runtime_core::RuntimeFailure {
    mutsuki_runtime_core::RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "host.load_plan.protocol_class",
        format!(
            "plugin.{}.protocol.{protocol_id}.{reason}",
            manifest.plugin_id
        ),
    ))
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
    let mut candidates = BTreeMap::<String, BTreeSet<String>>::new();
    for manifest in manifests {
        for runner in &manifest.provides.runners {
            for protocol_id in &runner.accepted_protocol_ids {
                candidates
                    .entry(protocol_id.clone())
                    .or_default()
                    .insert(runner.runner_id.clone());
            }
        }
    }
    for (protocol_id, protocol_candidates) in candidates {
        if runner_bindings.contains_key(&protocol_id) {
            continue;
        }
        if protocol_candidates.len() != 1 {
            return Err(runner_binding_invalid(
                &protocol_id,
                "",
                "ambiguous_runner_requires_explicit_binding",
            ));
        }
        runner_bindings.insert(
            protocol_id,
            protocol_candidates
                .into_iter()
                .next()
                .expect("one runner candidate exists"),
        );
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
        "profile:{}:{:?}:{}:{}:{}:extensions={:?}:events={}:{:?}:traces={}:{:?}:scheduler_details={}:dispatch_spans={}",
        profile.profile_id,
        profile.mode,
        profile.enabled_plugins.len(),
        deployment_count,
        capability_graph.active_capabilities.len(),
        profile.supported_extensions,
        profile.observability.events.capacity,
        profile.observability.events.overflow_policy,
        profile.observability.traces.capacity,
        profile.observability.traces.overflow_policy,
        profile.observability.detailed_scheduler_decisions,
        profile.observability.dispatch_spans,
    )
}

pub(crate) fn core_manifest(runner: RunnerDescriptor) -> PluginManifest {
    let protocol_classes = runner
        .accepted_protocol_ids
        .iter()
        .cloned()
        .map(|protocol_id| (protocol_id, ProtocolClass::Core))
        .collect();
    PluginManifest {
        plugin_id: "core".into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "core".into(),
            sha256: "sha256:core".into(),
            companion_artifacts: Vec::new(),
        },
        provides: PluginProvides {
            runners: vec![runner],
            protocol_classes,
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
