use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

#[test]
fn resource_manager_supports_value_ref_mmap_cow_and_exclusive_write_lease() {
    let mut resources = ResourceManager::new();
    let small = resources.pack_value("small.v1", json!({"a": 1})).unwrap();
    assert!(matches!(small, PackedValue::Inline(_)));
    let big = resources
        .pack_value("big.v1", json!({"blob": "x".repeat(5000)}))
        .unwrap();
    let value_ref = match big {
        PackedValue::Value(value_ref) => value_ref,
        _ => panic!("large value should be stored by ref"),
    };
    assert_eq!(
        resources.get_value(&value_ref).unwrap()["blob"]
            .as_str()
            .unwrap()
            .len(),
        5000
    );

    let resource = resources
        .create_mmap_resource("bytes.v1", b"abc".to_vec())
        .unwrap();
    assert_eq!(resources.read_resource(&resource).unwrap(), b"abc");
    let blob = resources.create_blob_resource("blob.v1", b"blob-data".to_vec());
    assert!(matches!(blob.access, ResourceAccess::Blob { .. }));
    assert_eq!(resources.read_resource(&blob).unwrap(), b"blob-data");
    let cow = resources.copy_on_write(&resource, b"xyz".to_vec()).unwrap();
    assert_ne!(cow.ref_id, resource.ref_id);
    let lease = resources
        .acquire_write_lease(&resource.ref_id, "runner-a", Some(5))
        .unwrap();
    let updated = resources
        .write_with_lease(&lease, b"def".to_vec(), 2)
        .unwrap();
    assert_eq!(updated.generation, resource.generation + 1);
    assert_eq!(resources.read_resource(&updated).unwrap(), b"def");
}

#[test]
fn resource_manager_owns_resource_cells_and_step_leases() {
    let mut resources = ResourceManager::new();
    let cell = resources.create_resource_cell(
        "cell:http:default",
        "http.connection_pool",
        "plugin-http",
        "http.connection_pool.v1",
        "drain",
    );
    assert_eq!(cell.cell_id, "cell:http:default");

    let shared = resources
        .acquire_resource_lease(
            &cell.cell_id,
            "task-http",
            "executor-http-1",
            "shared",
            Some(12),
        )
        .unwrap();
    assert_eq!(shared.borrower_task_id, "task-http");
    assert_eq!(shared.borrower_executor_id, "executor-http-1");

    let second_shared = resources
        .acquire_resource_lease(
            &cell.cell_id,
            "task-http-2",
            "executor-http-2",
            "shared",
            None,
        )
        .unwrap();
    assert!(
        resources
            .acquire_resource_lease(
                &cell.cell_id,
                "task-http-3",
                "executor-http-3",
                "exclusive",
                None
            )
            .is_err()
    );

    resources.release_resource_lease(&shared).unwrap();
    resources.release_resource_lease(&second_shared).unwrap();
    let exclusive = resources
        .acquire_resource_lease(
            &cell.cell_id,
            "task-http-3",
            "executor-http-3",
            "exclusive",
            None,
        )
        .unwrap();
    resources.release_resource_lease(&exclusive).unwrap();
}

#[test]
fn core_resource_facade_wraps_descriptor_and_lease_operations() {
    let plan = super::fixtures::load_plan(Vec::new(), Vec::new());
    let mut runtime = super::fixtures::boot_with_kernel(plan);
    let resource = runtime.create_blob_resource("bytes.v1", b"abc".to_vec());

    assert_eq!(runtime.open_resource(&resource.ref_id).unwrap(), resource);
    assert_eq!(runtime.map_resource(&resource.ref_id).unwrap(), resource);
    assert_eq!(runtime.read_resource(&resource.ref_id).unwrap(), b"abc");

    let mmap = runtime
        .create_mmap_resource("bytes.v1", b"abc".to_vec())
        .unwrap();
    let lease = runtime
        .lock_resource(&mmap.ref_id, "runner-a", Some(3))
        .unwrap();
    let updated = runtime.write_resource(&lease, b"def".to_vec()).unwrap();
    assert_eq!(runtime.read_resource(&updated.ref_id).unwrap(), b"def");

    let cell = runtime.create_resource_cell(
        "cell:http",
        "http.connection_pool",
        "plugin-http",
        "http.connection_pool.v1",
        "drain",
    );
    let resource_lease = runtime
        .acquire_resource_lease(&cell.cell_id, "task-http", "executor-http", "shared", None)
        .unwrap();
    runtime.release_resource_lease(&resource_lease).unwrap();
}

#[test]
fn resource_hub_routes_typed_resources_and_executes_lazy_plans() {
    let mut resources = ResourceManager::new();
    let text = resources
        .create_cow_state_resource("text_buffer", "text.v1", b"hello".to_vec())
        .unwrap();
    let ast = resources
        .create_snapshot_resource("ast_snapshot", "ast.v1", &text, b"ast".to_vec())
        .unwrap();
    let facts = resources
        .create_fact_resource("project_facts", "facts.v1", json!({"root": "."}))
        .unwrap();
    let stream = resources.create_stream_resource(
        "model_output_stream",
        "token.v1",
        "python.resource",
        "stream://model",
    );
    let capability = resources.create_capability_resource("db_pool", "db.pool.v1");

    assert_eq!(resources.resource_store_name(&text.ref_id), Some("cow"));
    assert_eq!(
        resources.resource_store_name(&ast.ref_id),
        Some("snapshots")
    );
    assert_eq!(resources.resource_store_name(&facts.ref_id), Some("facts"));
    assert_eq!(
        resources.resource_store_name(&stream.ref_id),
        Some("streams")
    );
    assert_eq!(
        resources.resource_store_name(&capability.ref_id),
        Some("capabilities")
    );

    let read_plan = resources.build_read_plan(&text.ref_id, "collect").unwrap();
    let write_plan = resources
        .build_write_plan(&text.ref_id, "fail", json!({"replace": "all"}))
        .unwrap();
    assert_eq!(resources.read_resource(&text).unwrap(), b"hello");
    assert_eq!(resources.collect_read_plan(&read_plan).unwrap(), b"hello");

    let receipt = resources
        .commit_write_plan(&write_plan, b"world".to_vec())
        .unwrap();
    let updated = receipt.resource_ref.unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.resource_id.version, 2);
    assert_eq!(resources.read_resource(&updated).unwrap(), b"world");

    let updated_read_plan = resources
        .build_read_plan(&updated.ref_id, "collect")
        .unwrap();
    let snapshot = resources
        .snapshot_read_plan(&updated_read_plan, "text_snapshot", "text.snapshot.v1")
        .unwrap();
    assert_eq!(snapshot.source_version, 2);
    assert_eq!(
        snapshot.snapshot_ref.semantic,
        ResourceSemantic::VersionedSnapshot
    );
    assert!(
        resources
            .open_stream_plan(&resources.build_read_plan(&stream.ref_id, "open").unwrap())
            .is_ok()
    );
    assert!(resources.open_stream_plan(&read_plan).is_err());
}

#[test]
fn resource_manager_executes_inline_utf8_export_plan() {
    let mut resources = ResourceManager::new();
    let resource = resources.create_blob_resource("text.v1", b"hello".to_vec());
    let plan = resources
        .build_export_plan(&resource.ref_id, "inline_utf8")
        .unwrap();

    let receipt = resources.execute_export_plan(&plan).unwrap();

    assert_eq!(receipt.plan_id, plan.plan_id);
    assert_eq!(receipt.status, "exported");
    assert_eq!(receipt.output, json!("hello"));
    assert_eq!(receipt.resource_ref.unwrap().ref_id, resource.ref_id);
}

#[test]
fn resource_manager_rejects_invalid_export_plans_loudly() {
    let mut resources = ResourceManager::new();
    let resource = resources.create_blob_resource("text.v1", b"hello".to_vec());
    let unsupported = resources
        .build_export_plan(&resource.ref_id, "json")
        .unwrap();
    assert_eq!(
        resources
            .execute_export_plan(&unsupported)
            .unwrap_err()
            .error()
            .code,
        "resource.export_unsupported"
    );

    let binary = resources.create_blob_resource("bytes.v1", vec![0xff]);
    let binary_export = resources
        .build_export_plan(&binary.ref_id, "inline_utf8")
        .unwrap();
    assert_eq!(
        resources
            .execute_export_plan(&binary_export)
            .unwrap_err()
            .error()
            .code,
        "resource.export_decode_failed"
    );

    let state = resources
        .create_cow_state_resource("text_buffer", "text.v1", b"old".to_vec())
        .unwrap();
    let stale_export = resources
        .build_export_plan(&state.ref_id, "inline_utf8")
        .unwrap();
    let write = resources
        .build_write_plan(&state.ref_id, "fail", json!({"replace": "all"}))
        .unwrap();
    resources
        .commit_write_plan(&write, b"new".to_vec())
        .unwrap();
    assert_eq!(
        resources
            .execute_export_plan(&stale_export)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_GENERATION_MISMATCH
    );
}

#[test]
fn resource_manager_executes_command_batch_and_saga_plans() {
    let mut resources = ResourceManager::new();
    let capability = resources.create_capability_resource("db_pool", "db.pool.v1");
    let command = resources
        .build_command_plan(
            &capability.ref_id,
            "query",
            json!({"sql": "select 1"}),
            Some("query:1".into()),
        )
        .unwrap();

    let receipt = resources.execute_command_plan(&command).unwrap();
    assert_eq!(receipt.status, "commanded");
    assert_eq!(receipt.resource_ref.unwrap(), capability);
    assert_eq!(receipt.output["operation"], "query");
    assert_eq!(receipt.output["idempotency_key"], "query:1");

    let batch = CommandBatch {
        batch_id: "batch:1".into(),
        commands: vec![command.clone(), command.clone()],
        rollback_guarantee: false,
    };
    assert_eq!(resources.execute_command_batch(&batch).unwrap().len(), 2);

    let rollback_batch = CommandBatch {
        rollback_guarantee: true,
        ..batch.clone()
    };
    assert_eq!(
        resources
            .execute_command_batch(&rollback_batch)
            .unwrap_err()
            .error()
            .code,
        "resource.rollback_unsupported"
    );

    let saga = SagaPlan {
        saga_id: "saga:ok".into(),
        steps: vec![command.clone(), command.clone()],
        compensations: vec![command.clone()],
    };
    assert_eq!(resources.execute_saga_plan(&saga).unwrap().len(), 2);

    let unsupported = resources
        .build_command_plan(&capability.ref_id, "drop", json!({}), None)
        .unwrap();
    assert_eq!(
        resources
            .execute_command_plan(&unsupported)
            .unwrap_err()
            .error()
            .code,
        "resource.command_unsupported"
    );

    let failed_saga = SagaPlan {
        saga_id: "saga:failed".into(),
        steps: vec![unsupported],
        compensations: vec![command],
    };
    let error = resources.execute_saga_plan(&failed_saga).unwrap_err();
    assert_eq!(error.error().code, "resource.saga_failed");
    assert_eq!(
        error.error().cause.as_ref().unwrap().code,
        "resource.command_unsupported"
    );
    assert_eq!(
        error.error().evidence.get("compensation_attempts"),
        Some(&ScalarValue::Int(1))
    );
}

#[test]
fn resource_manager_rejects_command_plan_for_non_capability_resource() {
    let mut resources = ResourceManager::new();
    let resource = resources.create_blob_resource("bytes.v1", b"abc".to_vec());
    let command = resources
        .build_command_plan(&resource.ref_id, "query", json!({}), None)
        .unwrap();

    assert_eq!(
        resources
            .execute_command_plan(&command)
            .unwrap_err()
            .error()
            .code,
        "resource.semantic_mismatch"
    );
}
