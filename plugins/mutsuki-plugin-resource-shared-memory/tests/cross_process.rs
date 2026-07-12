use std::process::Command;

use mutsuki_plugin_resource_shared_memory::SharedMemoryResourceProvider;
use mutsuki_runtime_contracts::ResourceAccess;
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
