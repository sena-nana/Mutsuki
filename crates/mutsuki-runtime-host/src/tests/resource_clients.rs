use std::io::Cursor;

use mutsuki_plugin_resource_memory::{MemoryResourceProvider, PROVIDER_ID};
use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::{AbiResourceClient, LocalResourceClient, ResourcePlanClient};

use super::helpers::test_resource_ref;

#[test]
fn host_resource_clients_execute_read_write_and_command_plans_across_backends() {
    let provider = MemoryResourceProvider::new();
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
    let local = LocalResourceClient::with_provider(PROVIDER_ID, provider);

    assert_eq!(local.collect_read_plan(&read_plan).unwrap(), b"hello");
    assert_eq!(
        mutsuki_runtime_sdk::ResourcePlanGateway::collect_read_plan(&local, &read_plan).unwrap(),
        b"hello"
    );
    assert_eq!(
        local.execute_export_plan(&export_plan).unwrap().output,
        json!("hello")
    );
    assert_eq!(
        local
            .commit_write_plan(&write_plan, b"new".to_vec())
            .unwrap()
            .new_version,
        Some(2)
    );
    let command_output = local.execute_command_plan(&command_plan).unwrap().output;
    assert_eq!(command_output["provider_id"], json!(PROVIDER_ID));
    assert_eq!(command_output["operation"], json!("query"));
    assert_eq!(command_output["args"], json!({"sql": "select 1"}));
    assert_eq!(command_output["idempotency_key"], json!("query:1"));
    assert_eq!(
        local
            .snapshot_read_plan(&read_plan, "text_snapshot", "text.snapshot.v1")
            .unwrap()
            .source_version,
        1
    );
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
