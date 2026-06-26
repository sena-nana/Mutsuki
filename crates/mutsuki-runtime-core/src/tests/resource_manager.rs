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
