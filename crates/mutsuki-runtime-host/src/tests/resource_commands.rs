use std::sync::Arc;

use mutsuki_plugin_resource_memory::{PLUGIN_ID, PROVIDER_ID};
use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, RuntimeBootstrapper};

use super::helpers::{host_with_echo_runner, runtime_profile};

#[test]
fn host_runtime_resource_commands_require_provider() {
    let runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let error = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
            provider_id: "host.resource_provider".into(),
            schema: "text.v1".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap_err();

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("provider_id"),
        Some(&ScalarValue::String("host.resource_provider".into()))
    );
}

#[test]
fn host_runtime_syncs_provider_commit_receipt_into_resource_registry() {
    let config = HostRuntimeConfig::default()
        .with_resource_provider(COMMAND_PROVIDER_ID, Arc::new(MalformedCommitProvider));
    let runtime = host_with_echo_runner()
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    let HostRuntimeReply::ResourceCreated(state) = runtime
        .dispatch(HostRuntimeCommand::CreateCowStateResource {
            provider_id: COMMAND_PROVIDER_ID.into(),
            kind_id: "text_buffer".into(),
            schema: "text.state.v1".into(),
            bytes: b"old".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected state resource");
    };
    let write = WritePlan {
        plan_id: "write:malformed".into(),
        resource: state.clone(),
        base_version: state.version,
        conflict_policy: "replace".into(),
        patch: PatchDescriptor {
            patch_id: "patch:malformed".into(),
            target_ref: state,
            base_version: 1,
            conflict_policy: "replace".into(),
            operations: json!({"replace": "new"}),
        },
        returning: None,
    };

    let error = runtime
        .dispatch(HostRuntimeCommand::CommitWritePlan {
            plan: Box::new(write),
            bytes: b"new".to_vec(),
        })
        .unwrap_err();

    assert_eq!(error.error().code, ERR_RESOURCE_GENERATION_MISMATCH);
}

#[test]
fn std_memory_provider_loads_through_host_and_syncs_resource_receipts() {
    let runtime = host_with_std_memory_provider();

    let HostRuntimeReply::ResourceCreated(blob) = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
            provider_id: PROVIDER_ID.into(),
            schema: "text.v1".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected memory blob resource");
    };
    assert_eq!(blob.provider_id, PROVIDER_ID);

    let HostRuntimeReply::ResourceBytes(bytes) = runtime
        .dispatch(HostRuntimeCommand::CollectReadPlan(Box::new(ReadPlan {
            plan_id: "read:memory".into(),
            resource: blob.clone(),
            operation: "collect".into(),
            args: json!(null),
        })))
        .unwrap()
    else {
        panic!("expected memory bytes");
    };
    assert_eq!(bytes, b"hello");

    let HostRuntimeReply::PlanReceipt(export_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteExportPlan(Box::new(
            ExportPlan {
                plan_id: "export:memory".into(),
                resource: blob.clone(),
                target: "inline_utf8".into(),
                args: json!(null),
            },
        )))
        .unwrap()
    else {
        panic!("expected memory export receipt");
    };
    assert_eq!(export_receipt.output, json!("hello"));

    let HostRuntimeReply::Snapshot(snapshot) = runtime
        .dispatch(HostRuntimeCommand::SnapshotReadPlan {
            plan: Box::new(ReadPlan {
                plan_id: "snapshot:memory".into(),
                resource: blob,
                operation: "collect".into(),
                args: json!(null),
            }),
            kind_id: "text_snapshot".into(),
            schema: "text.snapshot.v1".into(),
        })
        .unwrap()
    else {
        panic!("expected memory snapshot");
    };
    let HostRuntimeReply::ResourceDescriptor(snapshot_descriptor) = runtime
        .dispatch(HostRuntimeCommand::OpenResourceDescriptor(
            snapshot.snapshot_ref.ref_id.clone(),
        ))
        .unwrap()
    else {
        panic!("expected synced snapshot descriptor");
    };
    assert_eq!(
        snapshot_descriptor.semantic,
        ResourceSemantic::VersionedSnapshot
    );
}

#[test]
fn std_memory_provider_write_and_command_paths_sync_through_host() {
    let runtime = host_with_std_memory_provider();

    let HostRuntimeReply::ResourceCreated(state) = runtime
        .dispatch(HostRuntimeCommand::CreateCowStateResource {
            provider_id: PROVIDER_ID.into(),
            kind_id: "text_buffer".into(),
            schema: "text.state.v1".into(),
            bytes: b"old".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected memory state resource");
    };
    let write = WritePlan {
        plan_id: "write:memory".into(),
        resource: state.clone(),
        base_version: state.version,
        conflict_policy: "replace".into(),
        patch: PatchDescriptor {
            patch_id: "patch:memory".into(),
            target_ref: state.clone(),
            base_version: state.version,
            conflict_policy: "replace".into(),
            operations: json!({"replace": true}),
        },
        returning: None,
    };
    let HostRuntimeReply::PlanReceipt(write_receipt) = runtime
        .dispatch(HostRuntimeCommand::CommitWritePlan {
            plan: Box::new(write),
            bytes: b"new".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected memory write receipt");
    };
    assert_eq!(write_receipt.new_version, Some(2));

    let HostRuntimeReply::ResourceDescriptor(synced_state) = runtime
        .dispatch(HostRuntimeCommand::OpenResourceDescriptor(state.ref_id))
        .unwrap()
    else {
        panic!("expected synced state descriptor");
    };
    assert_eq!(synced_state.version, 2);
    assert_eq!(synced_state.resource_id.version, 2);

    let HostRuntimeReply::ResourceCreated(capability) = runtime
        .dispatch(HostRuntimeCommand::CreateCapabilityResource {
            provider_id: PROVIDER_ID.into(),
            kind_id: "memory_query".into(),
            schema: "memory.query.v1".into(),
        })
        .unwrap()
    else {
        panic!("expected memory capability");
    };
    let command = CommandPlan {
        plan_id: "command:memory".into(),
        capability,
        operation: "query".into(),
        args: json!({"sql": "select 1"}),
        idempotency_key: Some("query:memory".into()),
    };
    let HostRuntimeReply::PlanReceipt(command_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandPlan(Box::new(
            command.clone(),
        )))
        .unwrap()
    else {
        panic!("expected memory command receipt");
    };
    assert_eq!(command_receipt.output["provider_id"], PROVIDER_ID);

    let HostRuntimeReply::PlanReceipts(batch_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandBatch(Box::new(
            CommandBatch {
                batch_id: "batch:memory".into(),
                commands: vec![command.clone()],
                rollback_guarantee: false,
            },
        )))
        .unwrap()
    else {
        panic!("expected memory batch receipts");
    };
    assert_eq!(batch_receipts.len(), 1);

    let HostRuntimeReply::PlanReceipts(saga_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteSagaPlan(Box::new(SagaPlan {
            saga_id: "saga:memory".into(),
            steps: vec![command],
            compensations: Vec::new(),
        })))
        .unwrap()
    else {
        panic!("expected memory saga receipts");
    };
    assert_eq!(saga_receipts.len(), 1);
}

#[test]
fn host_runtime_rejects_created_descriptor_from_different_provider() {
    let config = HostRuntimeConfig::default()
        .with_resource_provider("mutsuki.host.mismatched", Arc::new(MalformedCommitProvider));
    let runtime = host_with_echo_runner()
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    let error = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
            provider_id: "mutsuki.host.mismatched".into(),
            schema: "text.v1".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap_err();

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

struct MalformedCommitProvider;

const COMMAND_PROVIDER_ID: &str = "mutsuki.host.command-provider";

fn host_with_std_memory_provider() -> crate::HostRuntime {
    let mut loader =
        BuiltinPluginLoader::new().with_plugin(Box::new(mutsuki_plugin_resource_memory::plugin()));
    let mut host = RuntimeBootstrapper::new();
    host.load_plugins(&mut loader).unwrap();
    host.into_host_runtime(RuntimeProfile {
        profile_id: "std-memory-provider".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    })
    .unwrap()
}

impl mutsuki_runtime_sdk::ResourcePlanGateway for MalformedCommitProvider {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Err(unused_test_provider_method("collect_read_plan"))
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(unused_test_provider_method("snapshot_read_plan"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unused_test_provider_method("open_stream_plan"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(unused_test_provider_method("execute_export_plan"))
    }

    fn commit_write_plan(&self, plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        let mut malformed = plan.resource.clone();
        malformed.version = plan.base_version + 1;
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "committed".into(),
            resource_ref: Some(malformed.clone()),
            snapshot: None,
            descriptor_updates: vec![malformed],
            new_version: Some(plan.base_version + 1),
            output: json!(null),
        })
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(unused_test_provider_method("execute_command_plan"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unused_test_provider_method("execute_command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unused_test_provider_method("execute_saga_plan"))
    }
}

impl mutsuki_runtime_sdk::ResourceProviderGateway for MalformedCommitProvider {
    fn create_blob_resource(&self, schema: &str, _bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        Ok(command_resource_ref(
            "provider:blob",
            "provider_blob",
            schema,
            ResourceSemantic::FrozenValue,
        ))
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        _bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Ok(command_resource_ref(
            "provider:cow",
            kind_id,
            schema,
            ResourceSemantic::CowVersionedState,
        ))
    }

    fn create_capability_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Err(unused_test_provider_method("create_capability_resource"))
    }
}

fn unused_test_provider_method(method: &str) -> mutsuki_runtime_core::RuntimeFailure {
    crate::error::host_failure(
        "host.test.unused_resource_provider_method",
        method.to_string(),
    )
}

fn command_resource_ref(
    ref_id: &str,
    kind_id: &str,
    schema: &str,
    semantic: ResourceSemantic,
) -> ResourceRef {
    ResourceRef {
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version: 1,
        },
        ref_id: ref_id.into(),
        semantic,
        provider_id: COMMAND_PROVIDER_ID.into(),
        resource_kind: kind_id.into(),
        schema: schema.into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: None,
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}
