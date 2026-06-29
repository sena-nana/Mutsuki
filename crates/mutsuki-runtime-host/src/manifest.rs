use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ArtifactType, LifecyclePolicy, PermissionGrant, PluginArtifact, PluginManifest, PluginProvides,
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
    PluginManifest {
        plugin_id: plugin_id.into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact,
        provides: PluginProvides {
            runners,
            ..PluginProvides::default()
        },
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
