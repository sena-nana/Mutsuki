#![cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use mutsuki_plugin_resource_shared_memory::{
    PROVIDER_ID, SharedMemoryResourceProvider, SharedMemoryView,
};
use mutsuki_runtime_contracts::{
    ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic,
};
use mutsuki_runtime_sdk::ResourceProviderGateway;

#[test]
fn child_process_opens_shared_memory_descriptor_by_name() {
    let provider = SharedMemoryResourceProvider::new();
    let blob = provider
        .create_blob_resource("text.v1", b"hello child process".to_vec())
        .unwrap();
    let ResourceAccess::SharedMemory { name, len, .. } = &blob.access else {
        panic!("expected shared-memory access");
    };

    let output = Command::new(env!("CARGO_BIN_EXE_mutsuki-shared-memory-child"))
        .arg(name)
        .arg(len.to_string())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"hello child process");
}

#[test]
fn mapped_view_remains_valid_after_owner_process_exits() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mutsuki-shared-memory-child"))
        .arg("--own")
        .arg("owner process exited")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut published = String::new();
    reader.read_line(&mut published).unwrap();
    let mut fields = published.split_whitespace();
    let name = fields.next().unwrap();
    let len: u64 = fields.next().unwrap().parse().unwrap();
    assert!(fields.next().is_none());

    let descriptor = foreign_descriptor(name, len);
    let view = SharedMemoryView::open(&descriptor).unwrap();
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());

    assert_eq!(view.bytes(), b"owner process exited");
}

fn foreign_descriptor(name: &str, len: u64) -> ResourceRef {
    ResourceRef {
        ref_id: "foreign-shared-memory".into(),
        resource_id: ResourceId {
            kind_id: "shared-memory-test".into(),
            slot_id: "foreign-shared-memory".into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: PROVIDER_ID.into(),
        resource_kind: "shared-memory-test".into(),
        schema: "bytes.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::SharedMemory {
            name: name.into(),
            offset: 0,
            len,
            readonly: true,
        },
        size_hint: Some(len),
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}
