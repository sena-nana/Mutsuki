//! 桌面 ABI v2 插件与 Core 公共加载器之间的单一适配边界。

use crate::plugin_package::PluginPackageRecord;
use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    ArtifactType, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SnapshotDescriptor, StreamPlan,
    TaskBatch, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_host::{AbiPluginLoadRequest, HostRuntime, load_abi_plugin_v2};
use mutsuki_runtime_sdk::{LoadedPlugin, ResourcePlanGateway, TaskSubmitter};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// 在 Runtime 启动前提供稳定回调对象，启动后绑定到 Actor client。
#[derive(Default)]
pub(crate) struct DeferredPluginHost {
    task_submitter: OnceLock<Arc<dyn TaskSubmitter>>,
    resource_gateway: OnceLock<Arc<dyn ResourcePlanGateway>>,
}

impl DeferredPluginHost {
    pub(crate) fn bind(&self, runtime: &HostRuntime) -> Result<(), String> {
        self.task_submitter
            .set(runtime.host_context().task_submitter_ref())
            .map_err(|_| "desktop ABI task submitter is already bound".to_string())?;
        self.resource_gateway
            .set(runtime.host_context().resource_gateway_ref())
            .map_err(|_| "desktop ABI resource gateway is already bound".to_string())
    }

    fn task_submitter(&self) -> RuntimeResult<&Arc<dyn TaskSubmitter>> {
        self.task_submitter.get().ok_or_else(deferred_not_bound)
    }

    fn resource_gateway(&self) -> RuntimeResult<&Arc<dyn ResourcePlanGateway>> {
        self.resource_gateway.get().ok_or_else(deferred_not_bound)
    }
}

pub(crate) fn connect_packaged_plugin(
    package: &PluginPackageRecord,
    host: Arc<DeferredPluginHost>,
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

impl TaskSubmitter for DeferredPluginHost {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.task_submitter()?.submit_batch(batch)
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.task_submitter()?.cancel_task(handle)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.task_submitter()?.task_outcome(handle)
    }
}

impl ResourcePlanGateway for DeferredPluginHost {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.resource_gateway()?.collect_read_plan(plan)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.resource_gateway()?
            .snapshot_read_plan(plan, kind_id, schema)
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.resource_gateway()?.open_stream_plan(plan)
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.resource_gateway()?.execute_export_plan(plan)
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.resource_gateway()?.commit_write_plan(plan, bytes)
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.resource_gateway()?.execute_command_plan(plan)
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.resource_gateway()?.execute_command_batch(batch)
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.resource_gateway()?.execute_saga_plan(saga)
    }
}

fn deferred_not_bound() -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "mutsuki_tauri_host.abi",
        "runtime_client.not_bound",
    ))
}
