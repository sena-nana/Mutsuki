use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ArtifactType, HostExtensionDescriptor, HostExtensionKind, LifecyclePolicy, PermissionGrant,
    PluginArtifact, PluginBackendDescriptor, PluginDeploymentKind, PluginManifest, PluginProvides,
    RunnerDescriptor,
};

pub fn runner_manifest(plugin_id: &str, runners: Vec<RunnerDescriptor>) -> PluginManifest {
    runner_manifest_with_artifact(
        plugin_id,
        PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "native".into(),
            sha256: "sha256:native".into(),
        },
        runners,
    )
}

pub fn runner_manifest_with_artifact(
    plugin_id: &str,
    artifact: PluginArtifact,
    runners: Vec<RunnerDescriptor>,
) -> PluginManifest {
    let provides = if matches!(&artifact.artifact_type, ArtifactType::Native) {
        native_runner_provides(plugin_id, runners)
    } else {
        PluginProvides {
            runners,
            ..PluginProvides::default()
        }
    };
    PluginManifest {
        plugin_id: plugin_id.into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact,
        provides,
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: Vec::new(),
            resources: Vec::new(),
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "drain_and_swap".into(),
            unload_timeout_ms: 5000,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: false,
        },
        metadata: BTreeMap::new(),
    }
}

fn native_runner_provides(plugin_id: &str, runners: Vec<RunnerDescriptor>) -> PluginProvides {
    let backend_id = format!("plugin.backend.{plugin_id}.builtin");
    PluginProvides {
        runners,
        host_extensions: vec![HostExtensionDescriptor {
            extension_id: format!("host.extension.{plugin_id}.builtin"),
            kind: HostExtensionKind::PluginBackend,
            supported_deployments: vec![PluginDeploymentKind::Builtin],
            reload_policy: "static".into(),
            drain_required: false,
        }],
        plugin_backends: vec![PluginBackendDescriptor {
            backend_id,
            deployment_kind: PluginDeploymentKind::Builtin,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: None,
            bridge_id: None,
        }],
        ..PluginProvides::default()
    }
}
