use std::sync::{Arc, Barrier};

use mutsuki_plugin_resource_sqlite::SqliteResourceProvider;
use mutsuki_runtime_contracts::{
    CommandPlan, ResourceProviderReply, ResourceProviderRequest, ResourceRef,
};
use mutsuki_runtime_sdk::ResourceProviderGateway;
use serde_json::json;

fn create_blob(provider: &SqliteResourceProvider, bytes: &[u8]) -> ResourceRef {
    let outcome = provider.execute(ResourceProviderRequest::CreateBlob {
        schema: "bytes.v1".into(),
        bytes: bytes.to_vec(),
    });
    match outcome.result.expect("blob creation succeeds") {
        ResourceProviderReply::Created(resource) => resource,
        other => panic!("unexpected provider reply: {other:?}"),
    }
}

fn create_capability(provider: &SqliteResourceProvider) -> ResourceRef {
    let outcome = provider.execute(ResourceProviderRequest::CreateCapability {
        kind_id: "sqlite_query".into(),
        schema: "sqlite.query.v1".into(),
    });
    match outcome.result.expect("capability creation succeeds") {
        ResourceProviderReply::Created(resource) => resource,
        other => panic!("unexpected provider reply: {other:?}"),
    }
}

#[test]
fn deleted_ids_are_not_reused_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("resources.db");
    let deleted = {
        let provider = SqliteResourceProvider::open(&path).unwrap();
        let capability = create_capability(&provider);
        let resource = create_blob(&provider, b"first");
        let outcome = provider.execute(ResourceProviderRequest::Command(CommandPlan {
            plan_id: "delete:resource".into(),
            capability,
            operation: "delete".into(),
            args: json!({"ref_id": resource.ref_id}),
            idempotency_key: None,
        }));
        outcome.result.expect("delete succeeds");
        resource.ref_id
    };

    let provider = SqliteResourceProvider::open(&path).unwrap();
    let created = create_blob(&provider, b"second");
    assert_ne!(created.ref_id, deleted);
}

#[test]
fn concurrent_provider_instances_allocate_distinct_ids() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("resources.db");
    let first = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let second = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let start = Arc::new(Barrier::new(3));

    let first_start = Arc::clone(&start);
    let first_provider = Arc::clone(&first);
    let first_thread = std::thread::spawn(move || {
        first_start.wait();
        create_blob(&first_provider, b"first")
    });
    let second_start = Arc::clone(&start);
    let second_provider = Arc::clone(&second);
    let second_thread = std::thread::spawn(move || {
        second_start.wait();
        create_blob(&second_provider, b"second")
    });

    start.wait();
    let first = first_thread.join().unwrap();
    let second = second_thread.join().unwrap();
    assert_ne!(first.ref_id, second.ref_id);
}
