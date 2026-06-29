use std::collections::{BTreeMap, BTreeSet};

use mutsuki_runtime_contracts::{
    ArtifactType, CapabilityProviderSelection, ContractSurface, ContractSurfaceKind,
    LifecyclePolicy, PermissionAuditEntry, PermissionGrant, PluginArtifact, PluginDeploymentKind,
    PluginManifest, PluginProvides, ResourceTypeDescriptor, RunnerDescriptor,
    RuntimeCapabilityGraph, RuntimeLoadPlan, RuntimeProfile, RuntimeProfileMode,
};
use mutsuki_runtime_core::RuntimeResult;

use crate::error::{
    capability_provider_missing, capability_version_mismatch, deployment_mismatch,
    permission_unauthorized, plugin_not_found, required_capability_missing, runner_binding_invalid,
};

#[derive(Default)]
struct ResolvedPlugins {
    manifests: Vec<PluginManifest>,
    deployments: BTreeMap<String, PluginDeploymentKind>,
}

#[derive(Clone, Debug)]
struct CapabilityProvider {
    provider_plugin_id: String,
    provider_version: Option<String>,
    surface_id: String,
}

#[derive(Clone, Debug)]
struct CapabilityRequirement {
    raw: String,
    capability: String,
    version_constraint: Option<String>,
}

pub fn resolve_load_plan(
    manifests: &[PluginManifest],
    profile: &RuntimeProfile,
) -> RuntimeResult<RuntimeLoadPlan> {
    let resolved = resolve_enabled_plugins(manifests, profile)?;
    let runner_bindings = runner_bindings(profile, &resolved.manifests)?;
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
    let mut required_raw = BTreeSet::new();
    let mut active = BTreeSet::new();
    let mut providers: BTreeMap<String, Vec<CapabilityProvider>> = BTreeMap::new();
    let mut active_resource_providers = BTreeSet::new();
    let mut active_host_backends = BTreeSet::new();
    let mut active_plugin_backends = BTreeSet::new();
    let mut active_codecs = BTreeSet::new();
    let mut active_bridges = BTreeSet::new();
    let mut active_scheduler_policies = BTreeSet::new();
    let mut active_workflows = BTreeSet::new();

    for manifest in manifests {
        required_raw.extend(manifest.requires.iter().cloned());
        collect_base_capabilities(manifest, &mut provided, &mut active, &mut providers);
        collect_system_extension_capabilities(manifest, &mut provided, &mut providers);
    }
    let requirements = parse_requirements(&required_raw);
    let required_capabilities: BTreeSet<String> = requirements
        .iter()
        .map(|requirement| requirement.capability.clone())
        .collect();

    for manifest in manifests {
        for backend in &manifest.provides.host_backends {
            if should_activate_extension(
                prune_extensions,
                &required_capabilities,
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
                || requires_capability(&required_capabilities, "resource_strategy", provider_id);
            if should_activate_extension(
                prune_extensions,
                &required_capabilities,
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
                &required_capabilities,
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
                &required_capabilities,
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
                &required_capabilities,
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
                &required_capabilities,
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
                &required_capabilities,
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

    for (prefix, active_ids) in [
        ("resource_provider", &active_resource_providers),
        ("host_backend", &active_host_backends),
        ("plugin_backend", &active_plugin_backends),
        ("bridge", &active_bridges),
        ("codec", &active_codecs),
        ("scheduler_policy", &active_scheduler_policies),
        ("workflow", &active_workflows),
    ] {
        validate_active_ids(prefix, active_ids, &providers)?;
    }
    validate_resource_type_providers(manifests, &active_resource_providers)?;
    ensure_required_capabilities_are_active(
        &requirements,
        &active,
        &active_resource_providers,
        &providers,
    )?;
    let active_capability_providers =
        active_capability_providers(&active, &requirements, &providers)?;
    let permission_audit = permission_audit_for(manifests, &active, &active_resource_providers)?;

    Ok(RuntimeCapabilityGraph {
        profile_mode: profile.mode.clone(),
        provided_capabilities: provided.into_iter().collect(),
        required_capabilities: required_raw.into_iter().collect(),
        active_capabilities: active.into_iter().collect(),
        active_capability_providers,
        active_resource_providers: active_resource_providers.into_iter().collect(),
        active_host_backends: active_host_backends.into_iter().collect(),
        active_plugin_backends: active_plugin_backends.into_iter().collect(),
        active_codecs: active_codecs.into_iter().collect(),
        active_bridges: active_bridges.into_iter().collect(),
        active_scheduler_policies: active_scheduler_policies.into_iter().collect(),
        active_workflows: active_workflows.into_iter().collect(),
        permission_audit,
    })
}

fn collect_base_capabilities(
    manifest: &PluginManifest,
    provided: &mut BTreeSet<String>,
    active: &mut BTreeSet<String>,
    providers: &mut BTreeMap<String, Vec<CapabilityProvider>>,
) {
    collect_active_capability(
        manifest,
        provided,
        active,
        providers,
        "plugin",
        &manifest.plugin_id,
        Some(manifest.version.clone()),
    );
    for runner in &manifest.provides.runners {
        collect_active_capability(
            manifest,
            provided,
            active,
            providers,
            "runner",
            &runner.runner_id,
            Some(runner.plugin_generation.to_string()),
        );
        for protocol_id in &runner.accepted_protocol_ids {
            collect_active_capability(
                manifest,
                provided,
                active,
                providers,
                "task_protocol",
                protocol_id,
                None,
            );
        }
    }
    for protocol in &manifest.provides.protocols {
        collect_active_capability(
            manifest,
            provided,
            active,
            providers,
            "protocol",
            &protocol.protocol_id,
            Some(protocol.version.clone()),
        );
    }
    for binding in &manifest.provides.handler_bindings {
        collect_active_capability(
            manifest,
            provided,
            active,
            providers,
            "handler_binding",
            &binding.binding_id,
            None,
        );
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
            collect_active_capability(manifest, provided, active, providers, prefix, name, None);
        }
    }
    for resource_type in &manifest.provides.resource_types {
        collect_active_capability(
            manifest,
            provided,
            active,
            providers,
            "resource_schema",
            &resource_type.kind_id,
            Some(resource_type.compatibility.schema_version.clone()),
        );
        collect_active_capability(
            manifest,
            provided,
            active,
            providers,
            "resource_type",
            &resource_type.kind_id,
            Some(resource_type.compatibility.schema_version.clone()),
        );
    }
}

fn collect_system_extension_capabilities(
    manifest: &PluginManifest,
    provided: &mut BTreeSet<String>,
    providers: &mut BTreeMap<String, Vec<CapabilityProvider>>,
) {
    for backend in &manifest.provides.host_backends {
        collect_provided_capability(
            manifest,
            provided,
            providers,
            "host_backend",
            &backend.backend_id,
            None,
        );
    }
    for backend in &manifest.provides.plugin_backends {
        collect_provided_capability(
            manifest,
            provided,
            providers,
            "plugin_backend",
            &backend.backend_id,
            None,
        );
    }
    for provider_id in &manifest.provides.resource_providers {
        collect_provided_capability(
            manifest,
            provided,
            providers,
            "resource_provider",
            provider_id,
            None,
        );
    }
    for codec in &manifest.provides.codecs {
        collect_provided_capability(
            manifest,
            provided,
            providers,
            "codec",
            &codec.codec_id,
            Some(codec.version.clone()),
        );
    }
    for bridge in &manifest.provides.bridges {
        collect_provided_capability(
            manifest,
            provided,
            providers,
            "bridge",
            &bridge.bridge_id,
            None,
        );
    }
    for policy in &manifest.provides.scheduler_policies {
        collect_provided_capability(
            manifest,
            provided,
            providers,
            "scheduler_policy",
            &policy.policy_id,
            Some(policy.version.clone()),
        );
    }
    for workflow in &manifest.provides.workflows {
        collect_provided_capability(
            manifest,
            provided,
            providers,
            "workflow",
            &workflow.workflow_id,
            None,
        );
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
    requirements: &[CapabilityRequirement],
    active: &BTreeSet<String>,
    active_resource_providers: &BTreeSet<String>,
    providers: &BTreeMap<String, Vec<CapabilityProvider>>,
) -> RuntimeResult<()> {
    for requirement in requirements {
        let Some(active_capability) =
            active_capability_for_requirement(requirement, active, active_resource_providers)
        else {
            return Err(required_capability_missing(&requirement.raw));
        };
        select_provider_for_requirement(&active_capability, requirement, providers)?;
    }
    Ok(())
}

fn active_capability_providers(
    active: &BTreeSet<String>,
    requirements: &[CapabilityRequirement],
    providers: &BTreeMap<String, Vec<CapabilityProvider>>,
) -> RuntimeResult<Vec<CapabilityProviderSelection>> {
    let mut selections = Vec::new();
    for capability in active {
        let matching_requirement = requirements
            .iter()
            .find(|requirement| requirement.capability == *capability);
        let provider = if let Some(requirement) = matching_requirement {
            select_provider_for_requirement(capability, requirement, providers)?
        } else {
            select_provider(capability, None, providers)?
        };
        selections.push(CapabilityProviderSelection {
            capability: capability.clone(),
            provider_plugin_id: provider.provider_plugin_id.clone(),
            provider_version: provider.provider_version.clone(),
            surface_id: provider.surface_id.clone(),
            reason: matching_requirement
                .and_then(|requirement| requirement.version_constraint.as_ref())
                .map(|_| "required_version")
                .unwrap_or("active_plan")
                .into(),
        });
    }
    Ok(selections)
}

fn select_provider_for_requirement<'a>(
    active_capability: &str,
    requirement: &CapabilityRequirement,
    providers: &'a BTreeMap<String, Vec<CapabilityProvider>>,
) -> RuntimeResult<&'a CapabilityProvider> {
    select_provider(
        active_capability,
        requirement.version_constraint.as_deref(),
        providers,
    )
    .map_err(|failure| {
        if requirement.version_constraint.is_some() {
            failure
        } else {
            required_capability_missing(&requirement.raw)
        }
    })
}

fn select_provider<'a>(
    capability: &str,
    version_constraint: Option<&str>,
    providers: &'a BTreeMap<String, Vec<CapabilityProvider>>,
) -> RuntimeResult<&'a CapabilityProvider> {
    let Some(candidates) = providers.get(capability) else {
        return Err(capability_provider_missing(capability));
    };
    for provider in candidates {
        if let Some(constraint) = version_constraint {
            let Some(version) = provider.provider_version.as_deref() else {
                return Err(capability_version_mismatch(capability, constraint, None));
            };
            if !version_matches_constraint(version, constraint) {
                continue;
            }
        }
        return Ok(provider);
    }
    Err(capability_version_mismatch(
        capability,
        version_constraint.expect("version constraint is present"),
        candidates
            .iter()
            .filter_map(|provider| provider.provider_version.as_deref())
            .max(),
    ))
}

fn active_capability_for_requirement(
    requirement: &CapabilityRequirement,
    active: &BTreeSet<String>,
    active_resource_providers: &BTreeSet<String>,
) -> Option<String> {
    if active.contains(&requirement.capability) {
        return Some(requirement.capability.clone());
    }
    if let Some(provider_id) = requirement.capability.strip_prefix("resource_strategy:")
        && active_resource_providers.contains(provider_id)
    {
        return Some(format!("resource_provider:{provider_id}"));
    }
    if requirement.capability.contains(':') {
        return None;
    }
    active.iter().find_map(|active_capability| {
        active_capability
            .rsplit_once(':')
            .and_then(|(_, id)| (id == requirement.capability).then(|| active_capability.clone()))
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

fn collect_active_capability(
    manifest: &PluginManifest,
    provided: &mut BTreeSet<String>,
    active: &mut BTreeSet<String>,
    providers: &mut BTreeMap<String, Vec<CapabilityProvider>>,
    prefix: &str,
    id: &str,
    provider_version: Option<String>,
) {
    collect_provided_capability(manifest, provided, providers, prefix, id, provider_version);
    insert_capability(active, prefix, id);
}

fn collect_provided_capability(
    manifest: &PluginManifest,
    provided: &mut BTreeSet<String>,
    providers: &mut BTreeMap<String, Vec<CapabilityProvider>>,
    prefix: &str,
    id: &str,
    provider_version: Option<String>,
) {
    insert_capability(provided, prefix, id);
    add_provider(providers, manifest, prefix, id, provider_version);
}

fn validate_active_ids(
    prefix: &str,
    active_ids: &BTreeSet<String>,
    providers: &BTreeMap<String, Vec<CapabilityProvider>>,
) -> RuntimeResult<()> {
    for active_id in active_ids {
        let capability = format!("{prefix}:{active_id}");
        if !providers.contains_key(&capability) {
            return Err(capability_provider_missing(&capability));
        }
    }
    Ok(())
}

fn validate_resource_type_providers(
    manifests: &[PluginManifest],
    active_resource_providers: &BTreeSet<String>,
) -> RuntimeResult<()> {
    for resource_type in manifests
        .iter()
        .flat_map(|manifest| manifest.provides.resource_types.iter())
    {
        if !active_resource_providers.contains(&resource_type.provider_id) {
            return Err(capability_provider_missing(&format!(
                "resource_provider:{}",
                resource_type.provider_id
            )));
        }
    }
    Ok(())
}

fn add_provider(
    providers: &mut BTreeMap<String, Vec<CapabilityProvider>>,
    manifest: &PluginManifest,
    prefix: &str,
    id: &str,
    provider_version: Option<String>,
) {
    let capability = format!("{prefix}:{id}");
    providers
        .entry(capability.clone())
        .or_default()
        .push(CapabilityProvider {
            provider_plugin_id: manifest.plugin_id.clone(),
            provider_version,
            surface_id: capability,
        });
}

fn parse_requirements(required: &BTreeSet<String>) -> Vec<CapabilityRequirement> {
    required
        .iter()
        .map(|raw| {
            let (capability, version_constraint) = raw
                .rsplit_once('@')
                .map(|(capability, constraint)| (capability.to_string(), Some(constraint.into())))
                .unwrap_or_else(|| (raw.clone(), None));
            CapabilityRequirement {
                raw: raw.clone(),
                capability,
                version_constraint,
            }
        })
        .collect()
}

fn version_matches_constraint(version: &str, constraint: &str) -> bool {
    let (operator, required_version) = parse_version_constraint(constraint);
    let Some(actual) = parse_version(version) else {
        return false;
    };
    let Some(required) = parse_version(required_version) else {
        return false;
    };
    match operator {
        "=" => actual == required,
        ">" => actual > required,
        ">=" => actual >= required,
        "<" => actual < required,
        "<=" => actual <= required,
        _ => false,
    }
}

fn parse_version_constraint(constraint: &str) -> (&str, &str) {
    for operator in [">=", "<=", ">", "<", "="] {
        if let Some(version) = constraint.strip_prefix(operator) {
            return (operator, version);
        }
    }
    ("=", constraint)
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn permission_audit_for(
    manifests: &[PluginManifest],
    active: &BTreeSet<String>,
    active_resource_providers: &BTreeSet<String>,
) -> RuntimeResult<Vec<PermissionAuditEntry>> {
    let mut audit = Vec::new();
    for manifest in manifests {
        for effect in &manifest.permissions.effects {
            let capability = format!("effect:{effect}");
            if !active.contains(&capability) {
                return Err(permission_unauthorized(
                    &manifest.plugin_id,
                    "effect",
                    effect,
                ));
            }
            audit.push(PermissionAuditEntry {
                plugin_id: manifest.plugin_id.clone(),
                permission_kind: "effect".into(),
                permission: effect.clone(),
                granted: true,
                provider_capability: Some(capability),
                reason: "active_effect".into(),
            });
        }
        for resource in &manifest.permissions.resources {
            let Some(capability) = resource_permission_capability(
                resource,
                manifests,
                active,
                active_resource_providers,
            ) else {
                return Err(permission_unauthorized(
                    &manifest.plugin_id,
                    "resource",
                    resource,
                ));
            };
            audit.push(PermissionAuditEntry {
                plugin_id: manifest.plugin_id.clone(),
                permission_kind: "resource".into(),
                permission: resource.clone(),
                granted: true,
                provider_capability: Some(capability),
                reason: "active_resource".into(),
            });
        }
    }
    Ok(audit)
}

fn resource_permission_capability(
    permission: &str,
    manifests: &[PluginManifest],
    active: &BTreeSet<String>,
    active_resource_providers: &BTreeSet<String>,
) -> Option<String> {
    if active.contains(permission) {
        return Some(permission.into());
    }
    for prefix in ["resource_provider", "resource_type", "resource_schema"] {
        let capability = format!("{prefix}:{permission}");
        if active.contains(&capability) {
            return Some(capability);
        }
    }
    if let Some((kind_id, operation)) = permission
        .strip_prefix("resource:")
        .and_then(|value| value.split_once(':'))
    {
        return active_resource_operation(
            Some(kind_id),
            operation,
            manifests,
            active_resource_providers,
        );
    }
    active_resource_operation(None, permission, manifests, active_resource_providers)
}

fn active_resource_operation(
    kind_id: Option<&str>,
    operation: &str,
    manifests: &[PluginManifest],
    active_resource_providers: &BTreeSet<String>,
) -> Option<String> {
    manifests
        .iter()
        .flat_map(|manifest| manifest.provides.resource_types.iter())
        .find(|resource_type| {
            kind_id.is_none_or(|kind_id| resource_type.kind_id == kind_id)
                && resource_type
                    .operations
                    .iter()
                    .any(|candidate| candidate == operation)
        })
        .and_then(|resource_type| {
            active_resource_type_capability(resource_type, active_resource_providers)
        })
}

fn active_resource_type_capability(
    resource_type: &ResourceTypeDescriptor,
    active_resource_providers: &BTreeSet<String>,
) -> Option<String> {
    active_resource_providers
        .contains(&resource_type.provider_id)
        .then(|| format!("resource_type:{}", resource_type.kind_id))
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
