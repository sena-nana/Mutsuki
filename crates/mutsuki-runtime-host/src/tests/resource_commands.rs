use std::sync::Arc;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::RuntimeResult;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply};

use super::helpers::{host_with_echo_runner, runtime_profile};

#[test]
fn host_runtime_resource_commands_require_provider() {
    let runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let error = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
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
fn host_runtime_resource_commands_use_injected_resource_provider() {
    let config = HostRuntimeConfig {
        resource_provider: Some(Arc::new(CommandResourceProvider)),
        ..HostRuntimeConfig::default()
    };
    let runtime = host_with_echo_runner()
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    let HostRuntimeReply::ResourceCreated(resource) = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
            schema: "text.v1".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected resource creation reply");
    };
    assert_eq!(resource.provider_id, "mutsuki.host.command-provider");
    let HostRuntimeReply::ResourceBytes(read_bytes) = runtime
        .dispatch(HostRuntimeCommand::CollectReadPlan(Box::new(ReadPlan {
            plan_id: "read:provider".into(),
            resource: resource.clone(),
            operation: "collect".into(),
            args: json!(null),
        })))
        .unwrap()
    else {
        panic!("expected resource bytes");
    };
    assert_eq!(read_bytes, b"provider-read");

    let HostRuntimeReply::Snapshot(snapshot) = runtime
        .dispatch(HostRuntimeCommand::SnapshotReadPlan {
            plan: Box::new(ReadPlan {
                plan_id: "snapshot:provider".into(),
                resource: resource.clone(),
                operation: "snapshot".into(),
                args: json!(null),
            }),
            kind_id: "provider.snapshot".into(),
            schema: "provider.snapshot.v1".into(),
        })
        .unwrap()
    else {
        panic!("expected snapshot");
    };
    assert_eq!(snapshot.source_ref.ref_id, resource.ref_id);

    let HostRuntimeReply::StreamPlan(stream) = runtime
        .dispatch(HostRuntimeCommand::OpenStreamPlan(Box::new(ReadPlan {
            plan_id: "stream:provider".into(),
            resource: resource.clone(),
            operation: "open_stream".into(),
            args: json!(null),
        })))
        .unwrap()
    else {
        panic!("expected stream plan");
    };
    assert_eq!(stream.plan_id, "stream:provider");

    let export = ExportPlan {
        plan_id: "export:provider".into(),
        resource,
        target: "inline_utf8".into(),
        args: json!(null),
    };

    let HostRuntimeReply::PlanReceipt(export_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteExportPlan(Box::new(export)))
        .unwrap()
    else {
        panic!("expected export receipt");
    };
    assert_eq!(export_receipt.output, json!("provider-export"));

    let HostRuntimeReply::ResourceCreated(capability) = runtime
        .dispatch(HostRuntimeCommand::CreateCapabilityResource {
            kind_id: "db_pool".into(),
            schema: "db.pool.v1".into(),
        })
        .unwrap()
    else {
        panic!("expected capability creation reply");
    };
    let command = CommandPlan {
        plan_id: "command:provider".into(),
        capability,
        operation: "query".into(),
        args: json!({"sql": "select 1"}),
        idempotency_key: Some("query:provider".into()),
    };

    let HostRuntimeReply::PlanReceipt(command_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandPlan(Box::new(
            command.clone(),
        )))
        .unwrap()
    else {
        panic!("expected command receipt");
    };
    assert_eq!(command_receipt.output, json!({"provider": "injected"}));

    let HostRuntimeReply::PlanReceipts(batch_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandBatch(Box::new(
            CommandBatch {
                batch_id: "batch:provider".into(),
                commands: vec![command.clone()],
                rollback_guarantee: false,
            },
        )))
        .unwrap()
    else {
        panic!("expected batch receipts");
    };
    assert_eq!(batch_receipts.len(), 1);

    let HostRuntimeReply::PlanReceipts(saga_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteSagaPlan(Box::new(SagaPlan {
            saga_id: "saga:provider".into(),
            steps: vec![command],
            compensations: Vec::new(),
        })))
        .unwrap()
    else {
        panic!("expected saga receipts");
    };
    assert_eq!(saga_receipts.len(), 1);
}

struct CommandResourceProvider;

impl mutsuki_runtime_sdk::ResourcePlanGateway for CommandResourceProvider {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Ok(b"provider-read".to_vec())
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Ok(SnapshotDescriptor {
            snapshot_ref: plan.resource.clone(),
            source_version: plan.resource.version,
            snapshot_version: 1,
            source_ref: plan.resource.clone(),
            is_stale: false,
            is_latest: true,
        })
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Ok(StreamPlan {
            plan_id: "stream:provider".into(),
            resource: plan.resource.clone(),
            operation: "open_stream".into(),
            args: json!(null),
        })
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "exported".into(),
            resource_ref: Some(plan.resource.clone()),
            snapshot: None,
            new_version: None,
            output: json!("provider-export"),
        })
    }

    fn commit_write_plan(&self, plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "committed".into(),
            resource_ref: Some(plan.resource.clone()),
            snapshot: None,
            new_version: Some(plan.base_version + 1),
            output: json!(null),
        })
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "commanded".into(),
            resource_ref: Some(plan.capability.clone()),
            snapshot: None,
            new_version: None,
            output: json!({"provider": "injected"}),
        })
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        batch
            .commands
            .iter()
            .map(|command| {
                mutsuki_runtime_sdk::ResourcePlanGateway::execute_command_plan(self, command)
            })
            .collect()
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        saga.steps
            .iter()
            .map(|command| {
                mutsuki_runtime_sdk::ResourcePlanGateway::execute_command_plan(self, command)
            })
            .collect()
    }
}

impl mutsuki_runtime_sdk::ResourceProviderGateway for CommandResourceProvider {
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
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Ok(command_resource_ref(
            "provider:capability",
            kind_id,
            schema,
            ResourceSemantic::CapabilityResource,
        ))
    }
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
        provider_id: "mutsuki.host.command-provider".into(),
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
