use super::*;
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode};
use mutsuki_runtime_host::{
    HostRuntime, HostRuntimeCommand as C, HostRuntimeConfig, HostRuntimeReply as R,
    RuntimeBootstrapper, TokioAsyncExecutor,
};
use mutsuki_runtime_sdk::{
    ResourceProviderOrdering, ResourceProviderOutcome, ResourceProviderReply as P,
    ResourceProviderRequest as Q,
};
use std::sync::{Arc, Barrier};
use std::time::Duration;

fn profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "invalidation".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![manifest().plugin_id],
        bindings: std::collections::BTreeMap::default(),
        surface_bindings: std::collections::BTreeMap::default(),
        supported_extensions: vec![],
        plugin_deployments: std::collections::BTreeMap::default(),
        observability: mutsuki_runtime_contracts::ObservabilityProfile::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
fn bootstrap(provider: Arc<dyn ResourceProviderGateway>) -> RuntimeBootstrapper {
    let mut bootstrap = RuntimeBootstrapper::new();
    bootstrap.register_manifest(manifest());
    bootstrap.register_resource_provider(PROVIDER_ID, provider);
    bootstrap
}
fn config() -> HostRuntimeConfig {
    HostRuntimeConfig::default().with_async_executor(Arc::new(
        TokioAsyncExecutor::new(2, 8, 8, 1024 * 1024).unwrap(),
    ))
}
fn runtime(provider: Arc<dyn ResourceProviderGateway>) -> HostRuntime {
    bootstrap(provider)
        .into_host_runtime_with_config(profile(), config())
        .unwrap()
}
fn create(runtime: &HostRuntime) -> ResourceRef {
    let R::ResourceCreated(resource) = runtime
        .dispatch(C::CreateBlobResource {
            provider_id: PROVIDER_ID.into(),
            schema: "bytes.v1".into(),
            bytes: vec![42; 10],
        })
        .unwrap()
    else {
        panic!("created");
    };
    resource
}
fn capability(runtime: &HostRuntime) -> ResourceRef {
    let R::ResourceCreated(resource) = runtime
        .dispatch(C::CreateCapabilityResource {
            provider_id: PROVIDER_ID.into(),
            kind_id: "sqlite_query".into(),
            schema: "sqlite.query.v1".into(),
        })
        .unwrap()
    else {
        panic!("created");
    };
    resource
}
fn delete(capability: &ResourceRef, resource: &ResourceRef) -> CommandPlan {
    CommandPlan {
        plan_id: format!("delete:{}", resource.ref_id),
        capability: capability.clone(),
        operation: "delete".into(),
        args: json!({"ref_id": resource.ref_id}),
        idempotency_key: None,
    }
}
fn absent(runtime: &HostRuntime, resource: &ResourceRef) {
    assert_eq!(
        runtime
            .dispatch(C::OpenResourceDescriptor(resource.ref_id.to_string()))
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_NOT_FOUND
    );
}
fn inventory(runtime: &HostRuntime) -> Vec<ResourceRef> {
    let R::ResourceDescriptors(resources) = runtime.dispatch(C::ResourceDescriptors).unwrap()
    else {
        panic!("inventory");
    };
    resources
}
fn same_inventory(runtime: &HostRuntime, provider: &SqliteResourceProvider) {
    let mut stored = provider.restore_descriptors().unwrap();
    stored.sort_by(|a, b| a.ref_id.cmp(&b.ref_id));
    assert_eq!(inventory(runtime), stored);
}

#[test]
fn retention_and_delete_are_immediately_invisible_and_restart_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resources.sqlite");
    let policy = SqliteRetentionConfig {
        max_age_seconds: Some(1),
        max_total_bytes: Some(16),
    };
    let provider = Arc::new(SqliteResourceProvider::open_with_retention(&path, policy).unwrap());
    let host = runtime(provider.clone());
    let cap = capability(&host);
    let first = create(&host);
    let second = create(&host);
    let third = create(&host);
    absent(&host, &first);
    same_inventory(&host, &provider);
    provider
        .state
        .lock()
        .unwrap()
        .connection
        .execute("UPDATE resources SET created_at_unix_ms = 1", [])
        .unwrap();
    let fresh = create(&host);
    absent(&host, &second);
    absent(&host, &third);
    host.dispatch(C::ExecuteCommandPlan(Box::new(delete(&cap, &fresh))))
        .unwrap();
    absent(&host, &fresh);
    host.dispatch(C::ExecuteCommandPlan(Box::new(delete(&cap, &cap))))
        .unwrap();
    absent(&host, &cap);
    same_inventory(&host, &provider);
    drop(host);
    drop(provider);
    let provider = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let host = runtime(provider.clone());
    for resource in [&first, &second, &third, &fresh, &cap] {
        absent(&host, resource);
    }
    assert!(inventory(&host).is_empty());
    let new = create(&host);
    assert_ne!(new.ref_id, fresh.ref_id);
    same_inventory(&host, &provider);
}

#[test]
fn ten_thousand_create_sweep_cycles_bound_the_live_hub() {
    let provider = Arc::new(
        SqliteResourceProvider::prepare(
            Connection::open_in_memory().unwrap(),
            SqliteRetentionConfig {
                max_age_seconds: None,
                max_total_bytes: Some(16),
            },
        )
        .unwrap(),
    );
    let host = runtime(provider.clone());
    let cap = capability(&host);
    let mut prior = Vec::new();
    for index in 0..10_000 {
        let resource = create(&host);
        if prior.len() == 2 {
            absent(&host, &prior.remove(0));
        }
        prior.push(resource);
        if index % 100 == 0 {
            same_inventory(&host, &provider);
            assert!(inventory(&host).len() <= 3);
        }
    }
    same_inventory(&host, &provider);
    assert_eq!(inventory(&host).len(), 3);
    assert!(
        host.dispatch(C::OpenResourceDescriptor(cap.ref_id.to_string()))
            .is_ok()
    );
}

#[test]
fn failed_batch_and_saga_preserve_committed_deletions() {
    let provider = Arc::new(SqliteResourceProvider::open_in_memory().unwrap());
    let host = runtime(provider.clone());
    let cap = capability(&host);
    let first = create(&host);
    let batch = CommandBatch {
        batch_id: "partial".into(),
        commands: vec![delete(&cap, &first), delete(&cap, &first)],
        rollback_guarantee: false,
    };
    assert!(
        host.dispatch(C::ExecuteCommandBatch(Box::new(batch)))
            .is_err()
    );
    absent(&host, &first);
    let second = create(&host);
    let compensation = create(&host);
    let saga = SagaPlan {
        saga_id: "partial".into(),
        steps: vec![delete(&cap, &second), delete(&cap, &second)],
        compensations: vec![delete(&cap, &first), delete(&cap, &compensation)],
    };
    assert!(host.dispatch(C::ExecuteSagaPlan(Box::new(saga))).is_err());
    absent(&host, &second);
    absent(&host, &compensation);
    same_inventory(&host, &provider);
}

#[test]
fn snapshot_retention_and_failed_insert_still_publish_removals() {
    let provider = Arc::new(
        SqliteResourceProvider::prepare(
            Connection::open_in_memory().unwrap(),
            SqliteRetentionConfig {
                max_age_seconds: Some(1),
                max_total_bytes: None,
            },
        )
        .unwrap(),
    );
    let host = runtime(provider.clone());
    let old = create(&host);
    provider
        .state
        .lock()
        .unwrap()
        .connection
        .execute("UPDATE resources SET created_at_unix_ms = 1", [])
        .unwrap();
    let R::Snapshot(snapshot) = host
        .dispatch(C::SnapshotReadPlan {
            plan: Box::new(ReadPlan {
                plan_id: "snapshot".into(),
                resource: old.clone(),
                operation: "snapshot".into(),
                args: Value::Null,
            }),
            kind_id: SNAPSHOT_KIND_ID.into(),
            schema: "bytes.v1".into(),
        })
        .unwrap()
    else {
        panic!("snapshot");
    };
    absent(&host, &old);
    same_inventory(&host, &provider);
    provider.state.lock().unwrap().connection.execute_batch("UPDATE resources SET created_at_unix_ms = 1; CREATE TRIGGER reject_insert BEFORE INSERT ON resources BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(
        host.dispatch(C::CreateBlobResource {
            provider_id: PROVIDER_ID.into(),
            schema: "bytes.v1".into(),
            bytes: vec![1]
        })
        .is_err()
    );
    absent(&host, &snapshot.snapshot_ref);
    same_inventory(&host, &provider);
}

struct PausedProvider {
    inner: Arc<SqliteResourceProvider>,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
    pause: std::sync::atomic::AtomicBool,
    calls: std::sync::atomic::AtomicUsize,
    restores: std::sync::atomic::AtomicUsize,
}
impl ResourceProviderGateway for PausedProvider {
    fn execute(&self, request: Q) -> ResourceProviderOutcome<P> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let outcome = self.inner.execute(request);
        if self.pause.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.entered.wait();
            self.release.wait();
        }
        outcome
    }
    fn restore_descriptors(&self) -> RuntimeResult<Vec<ResourceRef>> {
        self.restores
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.restore_descriptors()
    }
    fn execution(&self) -> ResourceProviderExecution {
        ResourceProviderExecution::Offloaded
    }
    fn ordering(&self) -> ResourceProviderOrdering {
        ResourceProviderOrdering::Ordered
    }
}

#[test]
fn timed_out_delete_is_applied_before_queued_create_across_staged_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resources.sqlite");
    let provider = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let paused = Arc::new(PausedProvider {
        inner: provider.clone(),
        entered: entered.clone(),
        release: release.clone(),
        pause: false.into(),
        calls: 0.into(),
        restores: 0.into(),
    });
    let mut cfg = config();
    cfg.default_runner_limits.wall_clock_deadline = Some(Duration::from_millis(100));
    let mut host = bootstrap(paused.clone())
        .into_host_runtime_with_config(profile(), cfg)
        .unwrap();
    let old = create(&host);
    let cap = capability(&host);
    paused
        .pause
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let client = host.host_context().resource_registry_ref();
    let plan = delete(&cap, &old);
    let worker = std::thread::spawn(move || client.execute_command_plan(&plan));
    entered.wait();
    // Actor remains responsive although the committed deletion has not been delivered yet.
    host.statistics().unwrap();
    assert!(worker.join().unwrap().is_err());
    let next_provider = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let candidate = Arc::new(PausedProvider {
        inner: next_provider.clone(),
        entered: Arc::new(Barrier::new(1)),
        release: Arc::new(Barrier::new(1)),
        pause: false.into(),
        calls: 0.into(),
        restores: 0.into(),
    });
    let old_instance = Arc::downgrade(&paused);
    drop(paused);
    let prepared = bootstrap(candidate.clone())
        .prepare_reload(profile(), 2)
        .unwrap();
    host.reload(prepared, Duration::from_secs(2)).unwrap();
    assert!(old_instance.upgrade().is_some());
    let client = host.host_context().resource_registry_ref();
    let next =
        std::thread::spawn(move || client.create_blob_resource(PROVIDER_ID, "bytes.v1", vec![7]));
    release.wait();
    let new = next.join().unwrap().unwrap();
    absent(&host, &old);
    assert_ne!(old.ref_id, new.ref_id);
    same_inventory(&host, &next_provider);
    assert_eq!(candidate.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        candidate.restores.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(old_instance.upgrade().is_none());
}

struct PanickingProvider(
    Arc<std::sync::atomic::AtomicUsize>,
    ResourceProviderExecution,
);
impl ResourceProviderGateway for PanickingProvider {
    fn execute(&self, _: Q) -> ResourceProviderOutcome<P> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("unknown provider effects");
    }
    fn execution(&self) -> ResourceProviderExecution {
        self.1
    }
    fn ordering(&self) -> ResourceProviderOrdering {
        ResourceProviderOrdering::Ordered
    }
}
#[test]
fn panic_poison_prevents_further_provider_execution() {
    for execution in [
        ResourceProviderExecution::Inline,
        ResourceProviderExecution::Offloaded,
    ] {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let host = runtime(Arc::new(PanickingProvider(calls.clone(), execution)));
        for _ in 0..2 {
            assert!(
                host.dispatch(C::CreateBlobResource {
                    provider_id: PROVIDER_ID.into(),
                    schema: "bytes.v1".into(),
                    bytes: vec![]
                })
                .is_err()
            );
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        host.statistics().unwrap();
    }
}

#[test]
fn ordered_queue_rejects_excess_work_without_blocking_actor() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let provider = Arc::new(PausedProvider {
        inner: Arc::new(SqliteResourceProvider::open_in_memory().unwrap()),
        entered: entered.clone(),
        release: release.clone(),
        pause: true.into(),
        calls: 0.into(),
        restores: 0.into(),
    });
    let mut cfg = config();
    cfg.actor_data_queue_limit = 1;
    cfg.async_executor = Some(Arc::new(
        TokioAsyncExecutor::new(2, 1, 1, 1024 * 1024).unwrap(),
    ));
    let host = bootstrap(provider)
        .into_host_runtime_with_config(profile(), cfg)
        .unwrap();
    let client = host.host_context().resource_registry_ref();
    let first =
        std::thread::spawn(move || client.create_blob_resource(PROVIDER_ID, "bytes.v1", vec![1]));
    entered.wait();
    let (tx, rx) = std::sync::mpsc::channel();
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let client = host.host_context().resource_registry_ref();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = client.create_blob_resource(PROVIDER_ID, "bytes.v1", vec![2]);
                tx.send(result).unwrap();
            })
        })
        .collect();
    let rejected = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("full queue rejects immediately");
    host.statistics().unwrap();
    release.wait();
    first.join().unwrap().unwrap();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(
        rejected.unwrap_err().error().code,
        mutsuki_runtime_contracts::ERR_CAPABILITY_EXHAUSTED
    );
    rx.recv().unwrap().unwrap();
    assert_eq!(inventory(&host).len(), 2);
}

#[test]
fn host_drop_drains_timed_out_provider_before_restart() {
    drain_timed_out_provider(false);
}

#[test]
fn host_drop_drains_timed_out_async_create_before_restart() {
    drain_timed_out_provider(true);
}

fn drain_timed_out_provider(creating: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resources.sqlite");
    let provider = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let paused = Arc::new(PausedProvider {
        inner: provider,
        entered: entered.clone(),
        release: release.clone(),
        pause: false.into(),
        calls: 0.into(),
        restores: 0.into(),
    });
    let mut cfg = config();
    cfg.default_runner_limits.wall_clock_deadline = Some(Duration::from_millis(100));
    let host = bootstrap(paused.clone())
        .into_host_runtime_with_config(profile(), cfg)
        .unwrap();
    let resource = create(&host);
    let cap = capability(&host);
    paused
        .pause
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let client = host.host_context().resource_registry_ref();
    let plan = delete(&cap, &resource);
    let async_client = host.host_context().async_resource_registry_ref().unwrap();
    let worker = std::thread::spawn(move || {
        if creating {
            futures::executor::block_on(async_client.create_blob_resource(
                PROVIDER_ID.into(),
                "blob.v1".into(),
                vec![42],
            ))
            .map(|_| ())
        } else {
            client.execute_command_plan(&plan).map(|_| ())
        }
    });
    entered.wait();
    let timed_out = worker.join().unwrap();
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(host);
        dropped_tx.send(()).unwrap();
    });
    let early_drop = dropped_rx.recv_timeout(Duration::from_millis(30));
    release.wait();
    assert!(timed_out.is_err());
    assert!(early_drop.is_err());
    dropped_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    dropper.join().unwrap();
    let restarted = runtime(Arc::new(SqliteResourceProvider::open(&path).unwrap()));
    if creating {
        assert_eq!(inventory(&restarted).len(), 3);
        assert!(
            inventory(&restarted)
                .iter()
                .any(|d| d.ref_id != resource.ref_id && d.ref_id != cap.ref_id)
        );
    } else {
        absent(&restarted, &resource);
    }
}

#[test]
fn bulk_capacity_sweep_is_transactional_and_preserves_the_exact_oldest_prefix() {
    let mut provider = SqliteResourceProvider::open_in_memory().unwrap();
    let created = |request| {
        let P::Created(resource) = provider.execute(request).result.unwrap() else {
            panic!("created")
        };
        resource
    };
    let cap = created(Q::CreateCapability {
        kind_id: "db".into(),
        schema: "db.v1".into(),
    });
    let empty = created(Q::CreateBlob {
        schema: "bytes.v1".into(),
        bytes: vec![],
    });
    let resources: Vec<_> = (0..10_000)
        .map(|_| {
            created(Q::CreateBlob {
                schema: "bytes.v1".into(),
                bytes: vec![1],
            })
        })
        .collect();
    provider.retention.max_total_bytes = Some(3);
    // A mid-sweep error rolls the capacity transaction back, including the
    // earlier zero-byte row; no uncommitted identity can reach the Host.
    provider
        .state
        .lock()
        .unwrap()
        .connection
        .execute_batch(&format!(
            "CREATE TRIGGER reject_bulk_delete BEFORE DELETE ON resources
         WHEN OLD.ref_id = '{}' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
            resources[5_000].ref_id
        ))
        .unwrap();
    let failed = provider.execute(Q::CreateBlob {
        schema: "bytes.v1".into(),
        bytes: vec![1],
    });
    assert!(failed.result.is_err());
    assert!(failed.invalidations.is_empty());
    assert_eq!(provider.restore_descriptors().unwrap().len(), 10_002);
    provider
        .state
        .lock()
        .unwrap()
        .connection
        .execute_batch("DROP TRIGGER reject_bulk_delete")
        .unwrap();
    let provider = Arc::new(provider);
    let host = runtime(provider.clone());
    let new = create(&host);
    same_inventory(&host, &provider);
    let live: std::collections::BTreeSet<_> =
        inventory(&host).into_iter().map(|r| r.ref_id).collect();
    let expected: std::collections::BTreeSet<_> = resources[9_997..]
        .iter()
        .map(|r| r.ref_id.clone())
        .chain([cap.ref_id, new.ref_id])
        .collect();
    assert_eq!(live, expected);
    absent(&host, &empty);
    absent(&host, &resources[9_996]);
}

#[test]
fn alternating_same_file_provider_instances_preserve_ids_and_live_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("alternating.sqlite");
    let providers: Vec<_> = (0..2)
        .map(|_| {
            Arc::new(PausedProvider {
                inner: Arc::new(
                    SqliteResourceProvider::open_with_retention(
                        &path,
                        SqliteRetentionConfig {
                            max_age_seconds: None,
                            max_total_bytes: Some(16),
                        },
                    )
                    .unwrap(),
                ),
                entered: Arc::new(Barrier::new(1)),
                release: Arc::new(Barrier::new(1)),
                pause: false.into(),
                calls: 0.into(),
                restores: 0.into(),
            })
        })
        .collect();
    let mut host = runtime(providers[0].clone());
    let mut ids = std::collections::BTreeSet::new();
    for index in 0..32 {
        let active = &providers[index % 2];
        if index > 0 {
            let candidate = bootstrap(active.clone())
                .prepare_reload(profile(), index as u64 + 1)
                .unwrap();
            host.reload(candidate, Duration::from_secs(2)).unwrap();
        }
        let resource = create(&host);
        assert!(ids.insert(resource.ref_id));
        same_inventory(&host, &active.inner);
        assert!(inventory(&host).len() <= 2);
    }
    for provider in &providers {
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 16);
    }
    assert_eq!(
        providers[0]
            .restores
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        providers[1]
            .restores
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[test]
fn async_large_blob_creation_bridges_sync_read_and_restores_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.sqlite");
    let large_config = || {
        HostRuntimeConfig::default().with_async_executor(Arc::new(
            TokioAsyncExecutor::new(2, 8, 8, 16 * 1024 * 1024).unwrap(),
        ))
    };
    let provider = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let host = bootstrap(provider.clone())
        .into_host_runtime_with_config(profile(), large_config())
        .unwrap();
    let registry = host.host_context().async_resource_registry_ref().unwrap();
    let bytes = vec![0x82; 8 * 1024 * 1024];
    let descriptor = futures::executor::block_on(registry.create_blob_resource(
        PROVIDER_ID.into(),
        "image/png".into(),
        bytes.clone(),
    ))
    .unwrap();
    assert_eq!(
        futures::executor::block_on(
            registry.open_resource_descriptor(descriptor.ref_id.to_string())
        )
        .unwrap(),
        descriptor
    );
    let plan = ReadPlan {
        plan_id: "large-consumer".into(),
        resource: descriptor.clone(),
        operation: "collect".into(),
        args: json!({}),
    };
    assert_eq!(
        host.host_context()
            .resource_gateway()
            .collect_read_plan(&plan)
            .unwrap(),
        bytes
    );
    same_inventory(&host, &provider);
    drop(registry);
    drop(host);
    drop(provider);
    let provider = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let host = bootstrap(provider.clone())
        .into_host_runtime_with_config(profile(), large_config())
        .unwrap();
    assert_eq!(
        host.host_context()
            .resource_registry()
            .open_resource_descriptor(descriptor.ref_id.as_str())
            .unwrap(),
        descriptor
    );
    assert_eq!(
        futures::executor::block_on(
            host.host_context()
                .async_resource_gateway()
                .unwrap()
                .collect_read_plan(plan)
        )
        .unwrap(),
        bytes
    );
    same_inventory(&host, &provider);
}

#[test]
fn timed_out_async_create_registers_across_reload_before_next_creation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("create-reload.sqlite");
    let inner = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let provider = Arc::new(PausedProvider {
        inner: inner.clone(),
        entered: entered.clone(),
        release: release.clone(),
        pause: true.into(),
        calls: 0.into(),
        restores: 0.into(),
    });
    let mut cfg = config();
    cfg.default_runner_limits.wall_clock_deadline = Some(Duration::from_millis(100));
    let mut host = bootstrap(provider.clone())
        .into_host_runtime_with_config(profile(), cfg)
        .unwrap();
    let registry = host.host_context().async_resource_registry_ref().unwrap();
    let client = registry.clone();
    let worker = std::thread::spawn(move || {
        futures::executor::block_on(client.create_blob_resource(
            PROVIDER_ID.into(),
            "blob.v1".into(),
            vec![42],
        ))
    });
    entered.wait();
    let timed_out = worker.join().unwrap();
    let stored = inner.restore_descriptors().unwrap();
    let before = inventory(&host);
    let replacement = Arc::new(SqliteResourceProvider::open(&path).unwrap());
    let reload = bootstrap(replacement.clone())
        .prepare_reload(profile(), 2)
        .unwrap();
    let reloaded = host.reload(reload, Duration::from_secs(2));
    release.wait();
    assert!(timed_out.is_err());
    assert!(before.is_empty());
    reloaded.unwrap();
    let next = futures::executor::block_on(registry.create_blob_resource(
        PROVIDER_ID.into(),
        "blob.v1".into(),
        vec![43],
    ))
    .unwrap();
    assert_ne!(next.ref_id, stored[0].ref_id);
    same_inventory(&host, &replacement);
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        provider.restores.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(inventory(&host).len(), 2);
}
