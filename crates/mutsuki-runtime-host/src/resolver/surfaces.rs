use mutsuki_runtime_contracts::{
    ContractSurface, ContractSurfaceKind, PluginManifest, RuntimeCapabilityGraph,
};

pub(super) fn surfaces_for(
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
        push_active_surface(
            surfaces,
            &capability_graph.active_resource_providers,
            provider_id,
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
    for extension in &manifest.provides.host_extensions {
        push_active_surface(
            surfaces,
            &capability_graph.active_host_extensions,
            &extension.extension_id,
            &manifest.plugin_id,
            ContractSurfaceKind::HostExtension,
            format!("host_extension:{}", extension.extension_id),
            format!(
                "host_extension:{}:{:?}:{}:{}",
                extension.extension_id,
                extension.kind,
                extension.reload_policy,
                extension.drain_required
            ),
        );
    }
    for backend in &manifest.provides.plugin_backends {
        push_active_surface(
            surfaces,
            &capability_graph.active_plugin_backends,
            &backend.backend_id,
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
        push_active_surface(
            surfaces,
            &capability_graph.active_codecs,
            &codec.codec_id,
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
        push_active_surface(
            surfaces,
            &capability_graph.active_bridges,
            &bridge.bridge_id,
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
        push_active_surface(
            surfaces,
            &capability_graph.active_scheduler_policies,
            &policy.policy_id,
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
        push_active_surface(
            surfaces,
            &capability_graph.active_workflows,
            &workflow.workflow_id,
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

fn push_active_surface(
    surfaces: &mut Vec<ContractSurface>,
    active_ids: &[String],
    id: &str,
    owner_plugin_id: &str,
    kind: ContractSurfaceKind,
    surface_id: String,
    fingerprint: String,
) {
    if active_surface(active_ids, id) {
        push_surface(surfaces, owner_plugin_id, kind, surface_id, fingerprint);
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
