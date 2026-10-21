//! 桌面 ABI v2 插件与 Core 公共加载器之间的单一适配边界。

use crate::plugin_package::PluginPackageRecord;
use mutsuki_runtime_contracts::ArtifactType;
use mutsuki_runtime_host::{AbiPluginLoadRequest, DeferredHostClient, load_abi_plugin_v2};
use mutsuki_runtime_sdk::{LoadedPlugin, ResourcePlanGateway, TaskSubmitter};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::Arc;

pub(crate) fn connect_packaged_plugin(
    package: &PluginPackageRecord,
    host: Arc<DeferredHostClient>,
) -> Result<LoadedPlugin, String> {
    let manifest = package
        .runtime_manifest
        .clone()
        .ok_or_else(|| format!("{} has no plugin.toml", package.plugin_id))?;
    if manifest.artifact.artifact_type != ArtifactType::Abi {
        return Err(format!(
            "desktop packaged artifact {:?} is not connected",
            manifest.artifact.artifact_type
        ));
    }
    let library_path = package
        .staged_artifact
        .clone()
        .ok_or_else(|| format!("{} has no staged ABI artifact", package.plugin_id))?;
    let config = abi_initialization_config(package, &manifest.artifact.path, &library_path)?;
    let task_submitter: Arc<dyn TaskSubmitter> = host.clone();
    let resource_gateway: Arc<dyn ResourcePlanGateway> = host;
    load_abi_plugin_v2(AbiPluginLoadRequest {
        library_path,
        expected_manifest: manifest,
        config: Some(config),
        task_submitter,
        resource_gateway,
    })
    .map_err(|error| format!("{:?}", error.error()))
}

fn abi_initialization_config(
    package: &PluginPackageRecord,
    artifact_path: &str,
    library_path: &Path,
) -> Result<Value, String> {
    let mut config = match package.config.clone() {
        Value::Null => Map::new(),
        Value::Object(config) => config,
        _ => return Err("ABI plugin initialization config must be an object".into()),
    };
    let depth = Path::new(artifact_path).components().count();
    let runtime_dir = library_path
        .ancestors()
        .nth(depth)
        .ok_or_else(|| format!("failed to resolve runtime directory for {artifact_path}"))?;
    let metadata = config
        .entry("_mutsuki")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(metadata) = metadata else {
        return Err("reserved _mutsuki initialization config must be an object".into());
    };
    metadata
        .entry("runtime_dir")
        .or_insert_with(|| json!(runtime_dir));
    metadata
        .entry("plugin_id")
        .or_insert_with(|| json!(package.plugin_id));
    metadata
        .entry("package_hash")
        .or_insert_with(|| json!(package.package_sha256));
    Ok(Value::Object(config))
}
