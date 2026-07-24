use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

#[test]
fn resource_manager_supports_value_refs_descriptors_and_write_lease_fencing() {
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
    assert_eq!(value_ref.content_hash, None);
    assert_eq!(
        resources.get_value(&value_ref).unwrap()["blob"]
            .as_str()
            .unwrap()
            .len(),
        5000
    );

    let resource = resources
        .register_resource_descriptor(external_resource_ref(
            "resource:bytes",
            "bytes",
            "bytes.v1",
            "mutsuki.std.resource.memory",
        ))
        .unwrap();
    assert_eq!(resources.open_resource(&resource.ref_id).unwrap(), resource);
    let inventory = resources.list_descriptors();
    assert!(
        inventory.iter().any(|item| item.ref_id == resource.ref_id),
        "list_descriptors should include registered resources"
    );
    assert_eq!(
        resources
            .register_resource_descriptor(resource.clone())
            .unwrap_err()
            .error()
            .code,
        ERR_CAPABILITY_EXHAUSTED
    );

    let mut stale_descriptor = external_resource_ref(
        "resource:stale",
        "bytes",
        "bytes.v1",
        "mutsuki.std.resource.memory",
    );
    stale_descriptor.resource_id.generation += 1;
    assert_eq!(
        resources
            .register_resource_descriptor(stale_descriptor)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_GENERATION_MISMATCH
    );

    let lease = resources
        .acquire_write_lease(&resource.ref_id, "runner-a", Some(5))
        .unwrap();
    assert_eq!(lease.token.generation, resource.generation);
    assert_eq!(
        resources.active_mutable_lease_routes_for_task("runner-a"),
        vec![format!("resource.write_lease.{}", lease.token.token_id)]
    );
    assert_eq!(
        resources
            .acquire_write_lease(&resource.ref_id, "runner-b", None)
            .unwrap_err()
            .error()
            .code,
        ERR_CAPABILITY_EXHAUSTED
    );

    let mut stale_lease = lease.clone();
    stale_lease.token.generation += 1;
    assert_eq!(
        resources
            .release_write_lease(&stale_lease)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_GENERATION_MISMATCH
    );
    resources.release_write_lease(&lease).unwrap();
    assert!(
        resources
            .active_mutable_lease_routes_for_task("runner-a")
            .is_empty()
    );

    let expiring = resources
        .acquire_write_lease(&resource.ref_id, "runner-a", Some(3))
        .unwrap();
    assert_eq!(
        resources
            .release_write_lease_at(&expiring, 3)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_LEASE_EXPIRED
    );
}

#[test]
fn resource_manager_syncs_provider_receipt_descriptor_updates() {
    let mut resources = ResourceManager::new();
    let resource = resources
        .register_resource_descriptor(external_resource_ref_with_semantic(
            "resource:text",
            "text_buffer",
            "text.v1",
            "mutsuki.std.resource.memory",
            ResourceSemantic::CowVersionedState,
            ResourceLifetime::Persistent,
        ))
        .unwrap();
    let mut updated = resource.clone();
    updated.version = 2;
    updated.resource_id.version = 2;
    updated.content_hash = Some("hash:v2".into());
    let receipt = PlanReceipt {
        plan_id: "write-plan:resource:text:1".into(),
        status: "committed".into(),
        resource_ref: Some(resource.clone()),
        snapshot: None,
        descriptor_updates: vec![updated.clone()],
        new_version: Some(2),
        output: json!({"accepted_bytes": 3}),
    };

    let synced = resources.sync_plan_receipt(&receipt).unwrap();

    assert_eq!(synced, vec![updated.clone()]);
    assert_eq!(resources.open_resource("resource:text").unwrap(), updated);

    let mut stale = resource.clone();
    stale.version = 1;
    stale.resource_id.version = 1;
    let stale_receipt = PlanReceipt {
        plan_id: "write-plan:resource:text:stale".into(),
        status: "committed".into(),
        resource_ref: Some(stale),
        snapshot: None,
        descriptor_updates: Vec::new(),
        new_version: Some(1),
        output: json!(null),
    };
    assert_eq!(
        resources
            .sync_plan_receipt(&stale_receipt)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_GENERATION_MISMATCH
    );
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
fn resource_manager_reclaims_expired_resource_leases() {
    let mut resources = ResourceManager::new();
    let cell =
        resources.create_resource_cell("cell:db", "db.pool", "plugin-db", "db.pool.v1", "drain");
    let lease = resources
        .acquire_resource_lease(
            &cell.cell_id,
            "task-db",
            "executor-db",
            "exclusive",
            Some(3),
        )
        .unwrap();

    assert_eq!(
        resources
            .release_resource_lease_at(&lease, 3)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_LEASE_EXPIRED
    );
    assert!(
        resources
            .acquire_resource_lease(
                &cell.cell_id,
                "task-other",
                "executor-other",
                "shared",
                None
            )
            .is_err()
    );

    let reclaimed = resources.reclaim_expired_resource_leases(3);
    assert_eq!(reclaimed, vec![lease]);
    let shared = resources
        .acquire_resource_lease(
            &cell.cell_id,
            "task-other",
            "executor-other",
            "shared",
            None,
        )
        .unwrap();
    resources.release_resource_lease(&shared).unwrap();
}

#[test]
fn resource_manager_rejects_stale_resource_lease_generation() {
    let mut resources = ResourceManager::new();
    let cell =
        resources.create_resource_cell("cell:db", "db.pool", "plugin-db", "db.pool.v1", "drain");
    let lease = resources
        .acquire_resource_lease(&cell.cell_id, "task-db", "executor-db", "shared", None)
        .unwrap();
    let mut stale = lease.clone();
    stale.generation += 1;

    assert_eq!(
        resources
            .release_resource_lease(&stale)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_GENERATION_MISMATCH
    );
    resources.release_resource_lease(&lease).unwrap();
}

#[test]
fn core_resource_facade_wraps_descriptor_and_lease_operations() {
    let plan = super::fixtures::load_plan(Vec::new(), Vec::new());
    let mut runtime = super::fixtures::boot_with_kernel(plan);
    let resource = runtime
        .register_resource_descriptor(super::fixtures::external_resource_ref(
            "resource:facade",
            "bytes",
            "bytes.v1",
            "mutsuki.std.resource.memory",
        ))
        .unwrap();

    assert_eq!(runtime.open_resource(&resource.ref_id).unwrap(), resource);
    assert_eq!(runtime.open_resource(&resource.ref_id).unwrap(), resource);

    let lease = runtime
        .lock_resource(&resource.ref_id, "runner-a", Some(3))
        .unwrap();
    assert_eq!(lease.token.ref_id, resource.ref_id);
    assert_eq!(lease.token.generation, resource.generation);
    runtime.release_write_lease(&lease).unwrap();

    let cell = runtime
        .create_resource_cell(
            "cell:http",
            "http.connection_pool",
            "plugin-http",
            "http.connection_pool.v1",
            "drain",
        )
        .unwrap();
    let resource_lease = runtime
        .acquire_resource_lease(&cell.cell_id, "task-http", "executor-http", "shared", None)
        .unwrap();
    runtime.release_resource_lease(&resource_lease).unwrap();

    let expiring = runtime
        .acquire_resource_lease(
            &cell.cell_id,
            "task-http",
            "executor-http",
            "shared",
            Some(0),
        )
        .unwrap();
    assert_eq!(
        runtime
            .release_resource_lease(&expiring)
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_LEASE_EXPIRED
    );
    assert_eq!(runtime.reclaim_expired_resource_leases(), vec![expiring]);
}

#[test]
fn resource_hub_routes_typed_resource_descriptors_and_builds_lazy_plans() {
    let mut resources = ResourceManager::new();
    let text = resources
        .register_resource_descriptor(external_resource_ref_with_semantic(
            "resource:text",
            "text_buffer",
            "text.v1",
            "mutsuki.std.resource.memory",
            ResourceSemantic::CowVersionedState,
            ResourceLifetime::Persistent,
        ))
        .unwrap();
    let ast = resources
        .register_resource_descriptor(external_resource_ref_with_semantic(
            "resource:ast",
            "ast_snapshot",
            "ast.v1",
            "mutsuki.std.resource.memory",
            ResourceSemantic::VersionedSnapshot,
            ResourceLifetime::Persistent,
        ))
        .unwrap();
    let facts = resources
        .register_resource_descriptor(external_resource_ref_with_semantic(
            "resource:facts",
            "project_facts",
            "facts.v1",
            "mutsuki.std.resource.memory",
            ResourceSemantic::ReadOnlyFact,
            ResourceLifetime::Persistent,
        ))
        .unwrap();
    let stream = resources.create_stream_resource(
        "model_output_stream",
        "token.v1",
        "python.resource",
        "stream://model",
    );
    let capability = resources
        .register_resource_descriptor(external_resource_ref_with_semantic(
            "resource:db_pool",
            "db_pool",
            "db.pool.v1",
            "mutsuki.std.resource.memory",
            ResourceSemantic::CapabilityResource,
            ResourceLifetime::ExternalManaged,
        ))
        .unwrap();

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
    let export_plan = resources
        .build_export_plan(&text.ref_id, "inline_utf8")
        .unwrap();
    let command_plan = resources
        .build_command_plan(
            &capability.ref_id,
            "query",
            json!({"sql": "select 1"}),
            Some("query:1".into()),
        )
        .unwrap();
    assert_eq!(read_plan.resource, text);
    assert_eq!(write_plan.resource.ref_id, text.ref_id);
    assert_eq!(write_plan.base_version, text.version);
    assert_eq!(write_plan.patch.target_ref.ref_id, text.ref_id);
    assert_eq!(export_plan.resource.ref_id, text.ref_id);
    assert_eq!(command_plan.capability.ref_id, capability.ref_id);
    assert!(
        resources
            .open_stream_plan(&resources.build_read_plan(&stream.ref_id, "open").unwrap())
            .is_ok()
    );
    assert!(resources.open_stream_plan(&read_plan).is_err());
}
