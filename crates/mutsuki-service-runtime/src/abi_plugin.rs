mod staging;

use std::sync::Arc;

use mutsuki_runtime_host::{AbiPluginLoadRequest, load_abi_plugin_v2};
use mutsuki_runtime_sdk::LoadedPlugin;
use mutsuki_service_config::ServiceConfig;
use mutsuki_service_plugin_loader::PluginRecord;
use serde_json::Value;

use self::staging::stage_artifact;
use super::{DeferredRuntimeClient, ServiceRuntimeError, ServiceRuntimeResult};

pub(crate) async fn load_abi_plugin(
    record: PluginRecord,
    config: ServiceConfig,
    runtime: Arc<DeferredRuntimeClient>,
    plugin_config: Value,
) -> ServiceRuntimeResult<LoadedPlugin> {
    let plugin_id = record.manifest.plugin_id.clone();
    tokio::task::spawn_blocking(move || {
        load_abi_plugin_blocking(record, config, runtime, plugin_config)
    })
    .await
    .map_err(|error| ServiceRuntimeError::AbiPlugin {
        plugin_id,
        detail: format!("ABI loading task failed: {error}"),
    })?
}

fn load_abi_plugin_blocking(
    record: PluginRecord,
    config: ServiceConfig,
    runtime: Arc<DeferredRuntimeClient>,
    plugin_config: Value,
) -> ServiceRuntimeResult<LoadedPlugin> {
    let source =
        record
            .resolved_artifact
            .as_ref()
            .ok_or_else(|| ServiceRuntimeError::AbiPlugin {
                plugin_id: record.manifest.plugin_id.clone(),
                detail: "validated artifact path is missing".into(),
            })?;
    let staged = stage_artifact(&config, &record, source)?;
    load_abi_plugin_v2(AbiPluginLoadRequest {
        library_path: staged,
        expected_manifest: record.manifest,
        config: Some(plugin_config),
        task_submitter: runtime.clone(),
        resource_gateway: runtime,
    })
    .map_err(ServiceRuntimeError::Core)
}

#[cfg(test)]
mod tests;
