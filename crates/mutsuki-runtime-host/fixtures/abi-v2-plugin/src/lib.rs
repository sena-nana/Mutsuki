//! Real ABI v2 fixture used by the generic dynamic-library loader tests.

use std::sync::Arc;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, PluginArtifact, ReadPlan,
    ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic,
    RunnerDescriptor, SnapshotDescriptor, StreamPlan, Task, WorkBatch, WritePlan,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AbiHostClientV2, PluginBuilder, ResourcePlanGateway, ResourceProviderGateway,
    RunnerDescriptorBuilder, TaskSubmitter, map_work_batch_entries,
};
use serde_json::{Value, json};

const PLUGIN_ID: &str = "mutsuki.test.runtime-host-abi-v2";
const RUNNER_ID: &str = "mutsuki.test.runtime-host-abi-v2.runner";
const PROVIDER_ID: &str = "mutsuki.test.runtime-host-abi-v2.provider";
const PROTOCOL_ID: &str = "mutsuki.test.runtime-host-abi-v2.echo";

struct FixtureRunner {
    descriptor: RunnerDescriptor,
    close_marker: Option<String>,
}

impl FixtureRunner {
    fn new(close_marker: Option<String>) -> Self {
        Self {
            descriptor: RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
                .accepted_protocol(PROTOCOL_ID)
                .build(),
            close_marker,
        }
    }
}

impl Drop for FixtureRunner {
    fn drop(&mut self) {
        if let Some(path) = &self.close_marker {
            let _ = std::fs::write(path, b"closed");
        }
    }
}

impl Runner for FixtureRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let mut result = mutsuki_runtime_contracts::RunnerResult::completed(&task.task_id);
            result.output = Some(json!({ "echo": task.payload }));
            Ok(result)
        })
    }
}

struct FixtureProvider;

impl ResourcePlanGateway for FixtureProvider {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Ok(b"fixture-resource".to_vec())
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(unsupported("snapshot"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unsupported("stream"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(unsupported("export"))
    }

    fn commit_write_plan(&self, _plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Err(unsupported("write"))
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(unsupported("command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported("command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported("saga"))
    }
}

impl ResourceProviderGateway for FixtureProvider {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        Ok(resource_ref(schema, bytes.len() as u64))
    }

    fn create_cow_state_resource(
        &self,
        _kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Ok(resource_ref(schema, bytes.len() as u64))
    }

    fn create_capability_resource(
        &self,
        _kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Ok(resource_ref(schema, 0))
    }
}

fn create_plugin(
    host: AbiHostClientV2,
    config: Value,
) -> RuntimeResult<mutsuki_runtime_sdk::LoadedPlugin> {
    if config.get("fail_initialize").and_then(Value::as_bool) == Some(true) {
        return Err(unsupported("initialize"));
    }
    if config.get("host_callback").and_then(Value::as_bool) == Some(true) {
        host.submit_one(Task::new(
            "abi-v2-fixture-host-callback",
            "mutsuki.test.host-callback",
            json!({ "source": PLUGIN_ID }),
        ))?;
    }
    let close_marker = config
        .get("close_marker")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut builder = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(FixtureRunner::new(close_marker)))
        .artifact(PluginArtifact {
            artifact_type: mutsuki_runtime_contracts::ArtifactType::Abi,
            path: "fixture".into(),
            sha256: "sha256:fixture".into(),
            companion_artifacts: Vec::new(),
        });
    if config.get("provider_mismatch").and_then(Value::as_bool) == Some(true) {
        builder = builder.resource_provider(PROVIDER_ID);
    } else {
        builder = builder.resource_provider_gateway(PROVIDER_ID, Arc::new(FixtureProvider));
    }
    Ok(builder.build())
}

fn resource_ref(schema: &str, size: u64) -> ResourceRef {
    ResourceRef {
        resource_id: ResourceId {
            kind_id: "fixture.blob".into(),
            slot_id: "fixture".into(),
            generation: 1,
            version: 1,
        },
        ref_id: "fixture.blob:fixture".into(),
        semantic: ResourceSemantic::FrozenValue,
        provider_id: PROVIDER_ID.into(),
        resource_kind: "fixture.blob".into(),
        schema: schema.into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: Some(size),
        content_hash: None,
        lifetime: ResourceLifetime::ExternalManaged,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn unsupported(route: &str) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        PLUGIN_ID,
        route,
    ))
}

mutsuki_runtime_sdk::export_mutsuki_plugin_abi_v2!(create_plugin);
