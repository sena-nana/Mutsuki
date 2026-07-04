use std::io::Cursor;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::RuntimeResult;
use serde_json::json;

use crate::{AbiResourceClient, LocalResourceClient, ResourcePlanClient};

use super::helpers::test_resource_ref;

const INJECTED_PROVIDER_ID: &str = "mutsuki.host.injected";

#[test]
fn host_resource_clients_execute_read_write_and_command_plans_across_backends() {
    let provider = InjectedResourceProvider;
    let blob = mutsuki_runtime_sdk::ResourceProviderGateway::create_blob_resource(
        &provider,
        "text.v1",
        b"hello".to_vec(),
    )
    .unwrap();
    let state = mutsuki_runtime_sdk::ResourceProviderGateway::create_cow_state_resource(
        &provider,
        "text_buffer",
        "text.state.v1",
        b"old".to_vec(),
    )
    .unwrap();
    let capability = mutsuki_runtime_sdk::ResourceProviderGateway::create_capability_resource(
        &provider,
        "db_pool",
        "db.pool.v1",
    )
    .unwrap();
    let read_plan = ReadPlan {
        plan_id: "read:1".into(),
        resource: blob.clone(),
        operation: "collect".into(),
        args: json!(null),
    };
    let write_plan = WritePlan {
        plan_id: "write:1".into(),
        resource: state.clone(),
        base_version: state.version,
        conflict_policy: "replace".into(),
        patch: PatchDescriptor {
            patch_id: "patch:1".into(),
            target_ref: state,
            base_version: 1,
            conflict_policy: "replace".into(),
            operations: json!({"replace": "all"}),
        },
        returning: None,
    };
    let export_plan = ExportPlan {
        plan_id: "export:1".into(),
        resource: blob,
        target: "inline_utf8".into(),
        args: json!(null),
    };
    let command_plan = CommandPlan {
        plan_id: "command:1".into(),
        capability,
        operation: "query".into(),
        args: json!({"sql": "select 1"}),
        idempotency_key: Some("query:1".into()),
    };
    let local = LocalResourceClient::with_provider(INJECTED_PROVIDER_ID, provider);

    assert_eq!(local.collect_read_plan(&read_plan).unwrap(), b"provider");
    assert_eq!(
        mutsuki_runtime_sdk::ResourcePlanGateway::collect_read_plan(&local, &read_plan).unwrap(),
        b"provider"
    );
    assert_eq!(
        local.execute_export_plan(&export_plan).unwrap().output,
        json!("provider")
    );
    assert_eq!(
        local
            .commit_write_plan(&write_plan, b"new".to_vec())
            .unwrap()
            .status,
        "committed"
    );
    assert_eq!(
        local.execute_command_plan(&command_plan).unwrap().output,
        json!({"provider": true})
    );
    assert_eq!(
        local
            .snapshot_read_plan(&read_plan, "text_snapshot", "text.snapshot.v1")
            .unwrap()
            .source_version,
        1
    );
    let mut stream_resource =
        test_resource_ref("stream:events", "events", ResourceSemantic::StreamResource);
    stream_resource.provider_id = INJECTED_PROVIDER_ID.into();
    let stream_plan = local
        .open_stream_plan(&ReadPlan {
            plan_id: "stream-read:1".into(),
            resource: stream_resource,
            operation: "open_stream".into(),
            args: json!(null),
        })
        .unwrap();
    assert_eq!(stream_plan.operation, "open_stream");
    assert_eq!(
        local
            .execute_command_batch(&CommandBatch {
                batch_id: "batch:1".into(),
                commands: vec![command_plan.clone()],
                rollback_guarantee: false,
            })
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        local
            .execute_saga_plan(&SagaPlan {
                saga_id: "saga:1".into(),
                steps: vec![command_plan.clone()],
                compensations: vec![command_plan],
            })
            .unwrap()
            .len(),
        1
    );

    let abi_receipt = PlanReceipt {
        plan_id: "abi-receipt".into(),
        status: "committed".into(),
        resource_ref: None,
        snapshot: None,
        descriptor_updates: Vec::new(),
        new_version: Some(2),
        output: json!(null),
    };
    let response = format!(
        "{}\n{}\n",
        json!({"id": "req-1", "ok": true, "result": [104, 101, 108, 108, 111]}),
        json!({"id": "req-2", "ok": true, "result": abi_receipt}),
    );
    let abi = AbiResourceClient::new(
        Cursor::new(response.into_bytes()),
        Cursor::new(Vec::<u8>::new()),
    );

    assert_eq!(abi.collect_read_plan(&read_plan).unwrap(), b"hello");
    assert_eq!(
        abi.commit_write_plan(&write_plan, b"new".to_vec())
            .unwrap()
            .new_version,
        Some(2)
    );
    let (_reader, writer) = abi.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert!(request.contains("\"method\":\"resource.read.collect\""));
    assert!(request.contains("\"method\":\"resource.write.commit\""));
    assert!(request.contains("\"bytes\":[110,101,119]"));
}

#[test]
fn resource_clients_implement_sdk_resource_gateway_boundary() {
    let resource = test_resource_ref("resource:text", "text", ResourceSemantic::FrozenValue);
    let read_plan = ReadPlan {
        plan_id: "read:1".into(),
        resource,
        operation: "collect".into(),
        args: json!(null),
    };
    let response = format!(
        "{}\n",
        json!({"id": "req-1", "ok": true, "result": [104, 101, 108, 108, 111]}),
    );
    let abi = AbiResourceClient::new(
        Cursor::new(response.into_bytes()),
        Cursor::new(Vec::<u8>::new()),
    );

    assert_eq!(
        mutsuki_runtime_sdk::ResourcePlanGateway::collect_read_plan(&abi, &read_plan).unwrap(),
        b"hello"
    );
}

#[test]
fn local_resource_client_accepts_injected_resource_provider() {
    let mut resource = test_resource_ref("resource:text", "text", ResourceSemantic::FrozenValue);
    resource.provider_id = INJECTED_PROVIDER_ID.into();
    let read_plan = ReadPlan {
        plan_id: "read:provider".into(),
        resource: resource.clone(),
        operation: "collect".into(),
        args: json!(null),
    };
    let write_plan = WritePlan {
        plan_id: "write:provider".into(),
        resource: resource.clone(),
        base_version: resource.version,
        conflict_policy: "replace".into(),
        patch: PatchDescriptor {
            patch_id: "patch:provider".into(),
            target_ref: resource,
            base_version: 1,
            conflict_policy: "replace".into(),
            operations: json!({"replace": true}),
        },
        returning: None,
    };
    let client = LocalResourceClient::with_provider(INJECTED_PROVIDER_ID, InjectedResourceProvider);

    assert_eq!(client.collect_read_plan(&read_plan).unwrap(), b"provider");
    assert_eq!(
        client
            .commit_write_plan(&write_plan, b"new".to_vec())
            .unwrap()
            .output,
        json!({"accepted_bytes": 3})
    );
}

#[test]
fn abi_resource_client_encodes_every_plan_method_surface() {
    let resource = test_resource_ref("resource:text", "text", ResourceSemantic::FrozenValue);
    let snapshot_ref = test_resource_ref(
        "resource:snapshot",
        "text_snapshot",
        ResourceSemantic::VersionedSnapshot,
    );
    let stream_ref = test_resource_ref("stream:events", "events", ResourceSemantic::StreamResource);
    let capability = test_resource_ref(
        "resource:db",
        "db_pool",
        ResourceSemantic::CapabilityResource,
    );
    let read_plan = ReadPlan {
        plan_id: "read:1".into(),
        resource: resource.clone(),
        operation: "collect".into(),
        args: json!(null),
    };
    let stream_read_plan = ReadPlan {
        plan_id: "stream-read:1".into(),
        resource: stream_ref.clone(),
        operation: "open_stream".into(),
        args: json!(null),
    };
    let export_plan = ExportPlan {
        plan_id: "export:1".into(),
        resource: resource.clone(),
        target: "inline_utf8".into(),
        args: json!(null),
    };
    let command_plan = CommandPlan {
        plan_id: "command:1".into(),
        capability: capability.clone(),
        operation: "query".into(),
        args: json!({"sql": "select 1"}),
        idempotency_key: Some("query:1".into()),
    };
    let snapshot = SnapshotDescriptor {
        snapshot_ref,
        source_version: 1,
        snapshot_version: 1,
        source_ref: resource,
        is_stale: false,
        is_latest: true,
    };
    let stream = StreamPlan {
        plan_id: "stream:1".into(),
        resource: stream_ref,
        operation: "open_stream".into(),
        args: json!(null),
    };
    let receipt = PlanReceipt {
        plan_id: "receipt:1".into(),
        status: "commanded".into(),
        resource_ref: Some(capability),
        snapshot: None,
        descriptor_updates: Vec::new(),
        new_version: None,
        output: json!({"ok": true}),
    };
    let response = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n",
        json!({"id": "req-1", "ok": true, "result": snapshot}),
        json!({"id": "req-2", "ok": true, "result": stream}),
        json!({"id": "req-3", "ok": true, "result": receipt.clone()}),
        json!({"id": "req-4", "ok": true, "result": receipt.clone()}),
        json!({"id": "req-5", "ok": true, "result": [receipt.clone()]}),
        json!({"id": "req-6", "ok": true, "result": [receipt]}),
    );
    let abi = AbiResourceClient::new(
        Cursor::new(response.into_bytes()),
        Cursor::new(Vec::<u8>::new()),
    );

    assert_eq!(
        abi.snapshot_read_plan(&read_plan, "text_snapshot", "text.snapshot.v1")
            .unwrap()
            .snapshot_version,
        1
    );
    assert_eq!(
        abi.open_stream_plan(&stream_read_plan).unwrap().operation,
        "open_stream"
    );
    assert_eq!(
        abi.execute_export_plan(&export_plan).unwrap().status,
        "commanded"
    );
    assert_eq!(
        abi.execute_command_plan(&command_plan).unwrap().status,
        "commanded"
    );
    assert_eq!(
        abi.execute_command_batch(&CommandBatch {
            batch_id: "batch:1".into(),
            commands: vec![command_plan.clone()],
            rollback_guarantee: false,
        })
        .unwrap()
        .len(),
        1
    );
    assert_eq!(
        abi.execute_saga_plan(&SagaPlan {
            saga_id: "saga:1".into(),
            steps: vec![command_plan.clone()],
            compensations: vec![command_plan],
        })
        .unwrap()
        .len(),
        1
    );

    let (_reader, writer) = abi.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert!(request.contains("\"method\":\"resource.read.snapshot\""));
    assert!(request.contains("\"method\":\"resource.stream.open\""));
    assert!(request.contains("\"method\":\"resource.export\""));
    assert!(request.contains("\"method\":\"resource.command\""));
    assert!(request.contains("\"method\":\"resource.command_batch\""));
    assert!(request.contains("\"method\":\"resource.saga\""));
}

struct InjectedResourceProvider;

impl mutsuki_runtime_sdk::ResourcePlanGateway for InjectedResourceProvider {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Ok(b"provider".to_vec())
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
            descriptor_updates: Vec::new(),
            new_version: None,
            output: json!("provider"),
        })
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        let mut updated = plan.resource.clone();
        updated.version = plan.base_version + 1;
        updated.resource_id.version = updated.version;
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "committed".into(),
            resource_ref: Some(updated.clone()),
            snapshot: None,
            descriptor_updates: vec![updated],
            new_version: Some(plan.base_version + 1),
            output: json!({ "accepted_bytes": bytes.len() }),
        })
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "commanded".into(),
            resource_ref: Some(plan.capability.clone()),
            snapshot: None,
            descriptor_updates: Vec::new(),
            new_version: None,
            output: json!({"provider": true}),
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

impl mutsuki_runtime_sdk::ResourceProviderGateway for InjectedResourceProvider {
    fn create_blob_resource(&self, schema: &str, _bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        Ok(injected_resource_ref(
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
        Ok(injected_resource_ref(
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
        Ok(injected_resource_ref(
            "provider:capability",
            kind_id,
            schema,
            ResourceSemantic::CapabilityResource,
        ))
    }
}

fn injected_resource_ref(
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
        provider_id: INJECTED_PROVIDER_ID.into(),
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
