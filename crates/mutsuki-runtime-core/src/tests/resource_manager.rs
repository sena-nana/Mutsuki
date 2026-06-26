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
