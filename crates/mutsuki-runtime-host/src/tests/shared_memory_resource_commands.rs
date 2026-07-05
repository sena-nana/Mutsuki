use mutsuki_plugin_resource_shared_memory::{
    PLUGIN_ID as SHARED_MEMORY_PLUGIN_ID, PROVIDER_ID as SHARED_MEMORY_PROVIDER_ID,
};
use mutsuki_runtime_contracts::*;
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn shared_memory_provider_loads_through_host_and_syncs_resource_receipts() {
    let runtime = host_with_shared_memory_provider();

    let HostRuntimeReply::ResourceCreated(blob) = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
            provider_id: SHARED_MEMORY_PROVIDER_ID.into(),
            schema: "text.v1".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected shared-memory blob resource");
    };
    assert_eq!(blob.provider_id, SHARED_MEMORY_PROVIDER_ID);
    assert!(matches!(blob.access, ResourceAccess::SharedMemory { .. }));

    let HostRuntimeReply::ResourceBytes(bytes) = runtime
        .dispatch(HostRuntimeCommand::CollectReadPlan(Box::new(ReadPlan {
            plan_id: "read:shared-memory".into(),
            resource: blob.clone(),
            operation: "collect".into(),
            args: json!(null),
        })))
        .unwrap()
    else {
        panic!("expected shared-memory bytes");
    };
    assert_eq!(bytes, b"hello");

    let HostRuntimeReply::PlanReceipt(export_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteExportPlan(Box::new(
            ExportPlan {
                plan_id: "export:shared-memory".into(),
                resource: blob.clone(),
                target: "inline_utf8".into(),
                args: json!(null),
            },
        )))
        .unwrap()
    else {
        panic!("expected shared-memory export receipt");
    };
    assert_eq!(export_receipt.output, json!("hello"));

    let HostRuntimeReply::Snapshot(snapshot) = runtime
        .dispatch(HostRuntimeCommand::SnapshotReadPlan {
            plan: Box::new(ReadPlan {
                plan_id: "snapshot:shared-memory".into(),
                resource: blob,
                operation: "collect".into(),
                args: json!(null),
            }),
            kind_id: "text_snapshot".into(),
            schema: "text.snapshot.v1".into(),
        })
        .unwrap()
    else {
        panic!("expected shared-memory snapshot");
    };
    assert!(matches!(
        snapshot.snapshot_ref.access,
        ResourceAccess::SharedMemory { .. }
    ));
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
fn shared_memory_provider_write_path_syncs_through_host() {
    let runtime = host_with_shared_memory_provider();

    let HostRuntimeReply::ResourceCreated(state) = runtime
        .dispatch(HostRuntimeCommand::CreateCowStateResource {
            provider_id: SHARED_MEMORY_PROVIDER_ID.into(),
            kind_id: "text_buffer".into(),
            schema: "text.state.v1".into(),
            bytes: b"old".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected shared-memory state resource");
    };
    let write = WritePlan {
        plan_id: "write:shared-memory".into(),
        resource: state.clone(),
        base_version: state.version,
        conflict_policy: "replace".into(),
        patch: PatchDescriptor {
            patch_id: "patch:shared-memory".into(),
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
        panic!("expected shared-memory write receipt");
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
    assert!(matches!(
        synced_state.access,
        ResourceAccess::SharedMemory { .. }
    ));
}

fn host_with_shared_memory_provider() -> crate::HostRuntime {
    let mut loader = BuiltinPluginLoader::new()
        .with_plugin(Box::new(mutsuki_plugin_resource_shared_memory::plugin()));
    let mut host = RuntimeBootstrapper::new();
    host.load_plugins(&mut loader).unwrap();
    host.into_host_runtime(RuntimeProfile {
        profile_id: "std-shared-memory-provider".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![SHARED_MEMORY_PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    })
    .unwrap()
}
