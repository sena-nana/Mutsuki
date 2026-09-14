use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AsyncResourcePlanGateway, AsyncResourceProviderGateway, BoxRuntimeFuture,
};
use serde_json::json;

use crate::{HostRuntimeConfig, RuntimeBootstrapper, TokioAsyncExecutor, runner_manifest};

use super::helpers::runtime_profile;

struct FakeAsyncProvider {
    provider_id: String,
    calls: Arc<AtomicUsize>,
}

#[allow(clippy::unused_self, clippy::unnecessary_wraps)]
impl FakeAsyncProvider {
    fn resource(&self, kind_id: &str, schema: &str) -> ResourceRef {
        ResourceRef {
            ref_id: format!("{}:{kind_id}", self.provider_id).into(),
            resource_id: ResourceId {
                kind_id: kind_id.into(),
                slot_id: format!("{}:{kind_id}", self.provider_id),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::CapabilityResource,
            provider_id: self.provider_id.clone(),
            resource_kind: kind_id.into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: self.provider_id.clone(),
                method: "execute".into(),
            },
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::ExternalManaged,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn unsupported<T>(operation: &str) -> BoxRuntimeFuture<T> {
        let operation = operation.to_string();
        Box::pin(async move {
            Err(RuntimeFailure::new(RuntimeError::new(
                "test.unsupported",
                "test.async_provider",
                operation,
            )))
        })
    }
}

impl AsyncResourcePlanGateway for FakeAsyncProvider {
    fn collect_read_plan(&self, _plan: ReadPlan) -> BoxRuntimeFuture<Vec<u8>> {
        Self::unsupported("collect_read_plan")
    }

    fn snapshot_read_plan(
        &self,
        _plan: ReadPlan,
        _kind_id: String,
        _schema: String,
    ) -> BoxRuntimeFuture<SnapshotDescriptor> {
        Self::unsupported("snapshot_read_plan")
    }

    fn open_stream_plan(&self, _plan: ReadPlan) -> BoxRuntimeFuture<StreamPlan> {
        Self::unsupported("open_stream_plan")
    }

    fn execute_export_plan(&self, _plan: ExportPlan) -> BoxRuntimeFuture<PlanReceipt> {
        Self::unsupported("execute_export_plan")
    }

    fn commit_write_plan(
        &self,
        _plan: WritePlan,
        _bytes: Vec<u8>,
    ) -> BoxRuntimeFuture<PlanReceipt> {
        Self::unsupported("commit_write_plan")
    }

    fn execute_command_plan(&self, plan: CommandPlan) -> BoxRuntimeFuture<PlanReceipt> {
        let provider_id = self.provider_id.clone();
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(PlanReceipt {
                plan_id: plan.plan_id,
                status: "completed".into(),
                resource_ref: None,
                snapshot: None,
                descriptor_updates: Vec::new(),
                new_version: None,
                output: json!({
                    "provider": provider_id,
                    "operation": plan.operation,
                }),
            })
        })
    }

    fn execute_command_batch(&self, _batch: CommandBatch) -> BoxRuntimeFuture<Vec<PlanReceipt>> {
        Self::unsupported("execute_command_batch")
    }

    fn execute_saga_plan(&self, _saga: SagaPlan) -> BoxRuntimeFuture<Vec<PlanReceipt>> {
        Self::unsupported("execute_saga_plan")
    }
}

#[allow(clippy::unused_self, clippy::unnecessary_wraps)]
impl FakeAsyncProvider {
    fn create_blob_resource(&self, schema: &str, _bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        Ok(self.resource("blob", schema))
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        _bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Ok(self.resource(kind_id, schema))
    }

    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Ok(self.resource(kind_id, schema))
    }
}

#[test]
fn fake_db_http_and_link_providers_support_async_and_sync_bridge() {
    let provider_ids = ["fake.db", "fake.http", "fake.link"];
    let calls = Arc::new(AtomicUsize::new(0));
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = provider_ids.iter().map(ToString::to_string).collect();
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(manifest);
    for provider_id in provider_ids {
        bootstrapper.register_async_resource_provider(
            provider_id,
            Arc::new(FakeAsyncProvider {
                provider_id: provider_id.into(),
                calls: calls.clone(),
            }),
        );
    }
    let config = HostRuntimeConfig::default().with_async_executor(Arc::new(
        TokioAsyncExecutor::new(2, 8, 8, 1024 * 1024).unwrap(),
    ));
    let runtime = bootstrapper
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    let plans: Vec<_> = provider_ids
        .iter()
        .map(|provider_id| {
            let capability = runtime
                .host_context()
                .resource_registry()
                .create_capability_resource(provider_id, "test.capability", "test.v1")
                .unwrap();
            CommandPlan {
                plan_id: format!("plan-{provider_id}"),
                capability,
                operation: "ping".into(),
                args: json!({}),
                idempotency_key: None,
            }
        })
        .collect();

    let bridged = runtime
        .host_context()
        .resource_gateway()
        .execute_command_plan(&plans[0])
        .unwrap();
    assert_eq!(bridged.output["provider"], provider_ids[0]);

    let gateway = runtime
        .host_context()
        .async_resource_gateway_ref()
        .expect("host should expose an async resource gateway");
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let receipts = tokio.block_on(async {
        let db = gateway.execute_command_plan(plans[0].clone());
        let http = gateway.execute_command_plan(plans[1].clone());
        let link = gateway.execute_command_plan(plans[2].clone());
        let (db, http, link) = futures::future::join3(db, http, link).await;
        vec![db.unwrap(), http.unwrap(), link.unwrap()]
    });

    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        receipts
            .iter()
            .map(|receipt| receipt.output["provider"].as_str().unwrap())
            .collect::<Vec<_>>(),
        provider_ids
    );
}

impl AsyncResourceProviderGateway for FakeAsyncProvider {
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderFuture {
        use mutsuki_runtime_sdk::{
            ResourceProviderOutcome as O, ResourceProviderReply as R, ResourceProviderRequest as Q,
        };
        let future: BoxRuntimeFuture<R> = match request {
            Q::CreateBlob { schema, bytes } => {
                let result = self.create_blob_resource(&schema, bytes).map(R::Created);
                Box::pin(async move { result })
            }
            Q::CreateCow {
                kind_id,
                schema,
                bytes,
            } => {
                let result = self
                    .create_cow_state_resource(&kind_id, &schema, bytes)
                    .map(R::Created);
                Box::pin(async move { result })
            }
            Q::CreateCapability { kind_id, schema } => {
                let result = self
                    .create_capability_resource(&kind_id, &schema)
                    .map(R::Created);
                Box::pin(async move { result })
            }
            Q::Collect(plan) => {
                let f = self.collect_read_plan(plan);
                Box::pin(async move { f.await.map(R::Bytes) })
            }
            Q::Snapshot {
                plan,
                kind_id,
                schema,
            } => {
                let f = self.snapshot_read_plan(plan, kind_id, schema);
                Box::pin(async move { f.await.map(|value| R::Snapshot(Box::new(value))) })
            }
            Q::OpenStream(plan) => {
                let f = self.open_stream_plan(plan);
                Box::pin(async move { f.await.map(R::Stream) })
            }
            Q::Export(plan) => {
                let f = self.execute_export_plan(plan);
                Box::pin(async move { f.await.map(|value| R::Receipt(Box::new(value))) })
            }
            Q::Commit { plan, bytes } => {
                let f = self.commit_write_plan(*plan, bytes);
                Box::pin(async move { f.await.map(|value| R::Receipt(Box::new(value))) })
            }
            Q::Command(plan) => {
                let f = self.execute_command_plan(plan);
                Box::pin(async move { f.await.map(|value| R::Receipt(Box::new(value))) })
            }
            Q::Batch(batch) => {
                let f = self.execute_command_batch(batch);
                Box::pin(async move { f.await.map(R::Receipts) })
            }
            Q::Saga(saga) => {
                let f = self.execute_saga_plan(saga);
                Box::pin(async move { f.await.map(R::Receipts) })
            }
        };
        Box::pin(async move { O::new(future.await) })
    }
}

struct RemovingAsyncProvider {
    descriptor: ResourceRef,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: AtomicUsize,
}
impl AsyncResourceProviderGateway for RemovingAsyncProvider {
    fn restore_descriptors(&self) -> RuntimeResult<Vec<ResourceRef>> {
        Ok(vec![self.descriptor.clone()])
    }
    fn ordering(&self) -> mutsuki_runtime_sdk::ResourceProviderOrdering {
        mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered
    }
    fn execute(
        &self,
        _request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderFuture {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        let entered = self.entered.clone();
        let release = self.release.clone();
        let descriptor = self.descriptor.clone();
        Box::pin(async move {
            if first {
                entered.notify_one();
                release.notified().await;
                mutsuki_runtime_sdk::ResourceProviderOutcome {
                    result: Err(RuntimeFailure::new(RuntimeError::new(
                        ERR_RESOURCE_NOT_FOUND,
                        "test.provider",
                        "partial.failure",
                    ))),
                    invalidations: vec![ResourceDescriptorInvalidation {
                        provider_id: descriptor.provider_id,
                        ref_id: descriptor.ref_id,
                        generation: descriptor.generation,
                    }],
                }
            } else {
                mutsuki_runtime_sdk::ResourceProviderOutcome::new(Ok(
                    mutsuki_runtime_sdk::ResourceProviderReply::Bytes(vec![]),
                ))
            }
        })
    }
}

#[test]
fn native_async_invalidation_survives_disconnected_caller_and_partial_failure() {
    let descriptor = FakeAsyncProvider {
        provider_id: "fake.lifecycle".into(),
        calls: Arc::new(AtomicUsize::new(0)),
    }
    .resource("capability", "fake.v1");
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let provider = Arc::new(RemovingAsyncProvider {
        descriptor: descriptor.clone(),
        entered: entered.clone(),
        release: release.clone(),
        calls: AtomicUsize::new(0),
    });
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec![descriptor.provider_id.clone()];
    let mut bootstrap = RuntimeBootstrapper::new();
    bootstrap.register_manifest(manifest);
    bootstrap.register_async_resource_provider(&descriptor.provider_id, provider);
    let host = bootstrap
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig::default().with_async_executor(Arc::new(
                TokioAsyncExecutor::new(2, 8, 8, 1024 * 1024).unwrap(),
            )),
        )
        .unwrap();
    let gateway = host.host_context().async_resource_gateway_ref().unwrap();
    let plan = ReadPlan {
        plan_id: "removal".into(),
        resource: descriptor.clone(),
        operation: "collect".into(),
        args: json!({}),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = gateway.clone();
        let request = plan.clone();
        let caller = tokio::spawn(async move { client.collect_read_plan(request).await });
        entered.notified().await;
        assert!(
            host.host_context()
                .resource_registry()
                .open_resource_descriptor(descriptor.ref_id.as_str())
                .is_ok()
        );
        caller.abort();
        let _ = caller.await;
        release.notify_one();
        // Next ordered operation completes only after the abandoned result is applied.
        gateway.collect_read_plan(plan).await.unwrap();
    });
    assert_eq!(
        host.host_context()
            .resource_registry()
            .open_resource_descriptor(descriptor.ref_id.as_str())
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_NOT_FOUND
    );
}

pub(super) struct FactoryAsyncProvider {
    pub(super) calls: Arc<AtomicUsize>,
    pub(super) panic: bool,
    pub(super) pause: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
impl AsyncResourceProviderGateway for FactoryAsyncProvider {
    fn ordering(&self) -> mutsuki_runtime_sdk::ResourceProviderOrdering {
        mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered
    }
    fn execute(
        &self,
        _: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic, "panic before Future construction");
        let pause = self.pause.clone();
        Box::pin(async move {
            if let Some((entered, release)) = pause {
                entered.notify_one();
                release.notified().await;
            }
            mutsuki_runtime_sdk::ResourceProviderOutcome::new(Ok(
                mutsuki_runtime_sdk::ResourceProviderReply::Bytes(vec![]),
            ))
        })
    }
}
fn factory_host(providers: Vec<(&str, FactoryAsyncProvider)>) -> crate::HostRuntime {
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = providers.iter().map(|(id, _)| (*id).into()).collect();
    let mut bootstrap = RuntimeBootstrapper::new();
    bootstrap.register_manifest(manifest);
    for (id, provider) in providers {
        bootstrap.register_async_resource_provider(id, Arc::new(provider));
    }
    bootstrap
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig::default().with_async_executor(Arc::new(
                TokioAsyncExecutor::new(2, 1, 8, 1024 * 1024).unwrap(),
            )),
        )
        .unwrap()
}
pub(super) fn factory_read(id: &str) -> ReadPlan {
    ReadPlan {
        plan_id: "factory-read".into(),
        resource: FakeAsyncProvider {
            provider_id: id.into(),
            calls: Arc::new(AtomicUsize::new(0)),
        }
        .resource("capability", "test.v1"),
        operation: "collect".into(),
        args: json!({}),
    }
}
#[test]
fn native_async_factory_runs_only_after_executor_admission() {
    let first_calls = Arc::new(AtomicUsize::new(0));
    let rejected_calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let host = factory_host(vec![
        (
            "busy",
            FactoryAsyncProvider {
                calls: first_calls.clone(),
                panic: false,
                pause: Some((entered.clone(), release.clone())),
            },
        ),
        (
            "rejected",
            FactoryAsyncProvider {
                calls: rejected_calls.clone(),
                panic: false,
                pause: None,
            },
        ),
    ]);
    let gateway = host.host_context().async_resource_gateway_ref().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let first_gateway = gateway.clone();
        let first =
            tokio::spawn(
                async move { first_gateway.collect_read_plan(factory_read("busy")).await },
            );
        entered.notified().await;
        let rejected = gateway.collect_read_plan(factory_read("rejected")).await;
        release.notify_one();
        first.await.unwrap().unwrap();
        assert_eq!(rejected.unwrap_err().error().code, ERR_CAPABILITY_EXHAUSTED);
        assert_eq!(rejected_calls.load(Ordering::SeqCst), 0);
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    });
}
#[test]
fn native_async_factory_panic_poison_does_not_kill_actor() {
    let calls = Arc::new(AtomicUsize::new(0));
    let host = factory_host(vec![(
        "panicking",
        FactoryAsyncProvider {
            calls: calls.clone(),
            panic: true,
            pause: None,
        },
    )]);
    let gateway = host.host_context().async_resource_gateway_ref().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert!(
            gateway
                .collect_read_plan(factory_read("panicking"))
                .await
                .is_err()
        );
        host.statistics().unwrap();
        assert_eq!(
            gateway
                .collect_read_plan(factory_read("panicking"))
                .await
                .unwrap_err()
                .error()
                .code,
            ERR_RUNTIME_HOST_FAILED
        );
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn large_outcome_deletions_dominate_all_receipt_updates_without_tombstones() {
    let mut bootstrap = RuntimeBootstrapper::new();
    bootstrap.register_manifest(runner_manifest("plugin-a", Vec::new()));
    let mut core = bootstrap.into_runtime(runtime_profile()).unwrap();
    let mut invalidations = Vec::new();
    let mut receipts = Vec::new();
    for index in 0..10_000 {
        let mut resource = factory_read("batch").resource;
        resource.ref_id = format!("resource-{index}").into();
        resource.resource_id.slot_id = resource.ref_id.to_string();
        core.register_resource_descriptor(resource.clone()).unwrap();
        invalidations.push(ResourceDescriptorInvalidation {
            provider_id: "batch".into(),
            ref_id: resource.ref_id.clone(),
            generation: 1,
        });
        receipts.push(PlanReceipt {
            plan_id: format!("receipt-{index}"),
            status: "updated".into(),
            resource_ref: Some(resource.clone()),
            snapshot: None,
            descriptor_updates: vec![resource],
            new_version: None,
            output: json!(null),
        });
    }
    let result = crate::resource_router::apply_resource_outcome(
        &mut core,
        "batch",
        mutsuki_runtime_sdk::ResourceProviderOutcome {
            result: Ok(crate::HostRuntimeReply::PlanReceipts(receipts)),
            invalidations,
        },
    )
    .unwrap();
    let crate::HostRuntimeReply::PlanReceipts(receipts) = result else {
        panic!("expected receipts")
    };
    assert!(
        receipts
            .iter()
            .all(|r| r.resource_ref.is_none() && r.descriptor_updates.is_empty())
    );
    assert!(core.resources().list_descriptors().is_empty());
}

#[test]
fn targeted_reload_replaces_only_affected_provider_routes_and_failed_reload_keeps_them() {
    let profile = || {
        let mut profile = runtime_profile();
        profile.enabled_plugins.push("plugin-b".into());
        profile
    };
    let builder = |calls: &[Arc<AtomicUsize>; 2], register_second: bool| {
        let mut bootstrap = RuntimeBootstrapper::new();
        for (index, id) in ["a", "b"].into_iter().enumerate() {
            let mut manifest = runner_manifest(&format!("plugin-{id}"), Vec::new());
            manifest.provides.resource_providers = vec![id.into()];
            bootstrap.register_manifest(manifest);
            if index == 0 || register_second {
                bootstrap.register_async_resource_provider(
                    id,
                    Arc::new(FactoryAsyncProvider {
                        calls: calls[index].clone(),
                        panic: false,
                        pause: None,
                    }),
                );
            }
        }
        bootstrap
    };
    let old = std::array::from_fn(|_| Arc::new(AtomicUsize::new(0)));
    let new = std::array::from_fn(|_| Arc::new(AtomicUsize::new(0)));
    let mut host = builder(&old, true)
        .into_host_runtime_with_config(
            profile(),
            HostRuntimeConfig::default().with_async_executor(Arc::new(
                TokioAsyncExecutor::new(2, 8, 8, 1024 * 1024).unwrap(),
            )),
        )
        .unwrap();
    let gateway = host.host_context().async_resource_gateway_ref().unwrap();
    let candidate = builder(&new, true)
        .prepare_targeted_reload_with_runner_limits(
            profile(),
            2,
            Default::default(),
            ["plugin-a".into()].into(),
        )
        .unwrap();
    host.reload(candidate, Duration::from_secs(2)).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        gateway.collect_read_plan(factory_read("a")).await.unwrap();
        gateway.collect_read_plan(factory_read("b")).await.unwrap();
    });
    assert_eq!(old[0].load(Ordering::SeqCst), 0);
    assert_eq!(old[1].load(Ordering::SeqCst), 1);
    assert_eq!(new[0].load(Ordering::SeqCst), 1);
    assert_eq!(new[1].load(Ordering::SeqCst), 0);
    let missing = builder(&old, false).prepare_reload(profile(), 3).unwrap();
    assert!(host.reload(missing, Duration::from_secs(2)).is_err());
    assert_eq!(host.host_context().registry_generation(), 2);
    runtime
        .block_on(gateway.collect_read_plan(factory_read("a")))
        .unwrap();
    assert_eq!(old[0].load(Ordering::SeqCst), 0);
    assert_eq!(new[0].load(Ordering::SeqCst), 2);
}

struct ConcurrentPausedProvider {
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Semaphore>,
}
impl AsyncResourceProviderGateway for ConcurrentPausedProvider {
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderFuture {
        let entered = self.entered.clone();
        let release = self.release.clone();
        Box::pin(async move {
            entered.send(()).unwrap();
            release.acquire().await.unwrap().forget();
            let reply = match request {
                mutsuki_runtime_sdk::ResourceProviderRequest::CreateCapability {
                    kind_id,
                    schema,
                } => mutsuki_runtime_sdk::ResourceProviderReply::Created(
                    FakeAsyncProvider {
                        provider_id: "changing".into(),
                        calls: Arc::new(AtomicUsize::new(0)),
                    }
                    .resource(&kind_id, &schema),
                ),
                _ => mutsuki_runtime_sdk::ResourceProviderReply::Bytes(vec![]),
            };
            mutsuki_runtime_sdk::ResourceProviderOutcome::new(Ok(reply))
        })
    }
}
#[test]
fn switching_concurrent_provider_to_ordered_waits_for_every_old_invocation() {
    concurrent_reload_fence(false);
}

#[test]
fn switching_to_ordered_retains_every_old_create_through_registration() {
    concurrent_reload_fence(true);
}

fn concurrent_reload_fence(create: bool) {
    let (entered, mut started) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let old = Arc::new(ConcurrentPausedProvider {
        entered,
        release: release.clone(),
    });
    let weak = Arc::downgrade(&old);
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec!["changing".into()];
    let mut bootstrap = RuntimeBootstrapper::new();
    bootstrap.register_manifest(manifest.clone());
    bootstrap.register_async_resource_provider("changing", old);
    let mut config = HostRuntimeConfig::default().with_async_executor(Arc::new(
        TokioAsyncExecutor::new(2, 4, 8, 1024 * 1024).unwrap(),
    ));
    config.actor_data_queue_limit = 1;
    let mut host = bootstrap
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    let gateway = host.host_context().async_resource_gateway_ref().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (old_tx, mut old_rx) = tokio::sync::mpsc::unbounded_channel();
        for index in 0..2 {
            let client = gateway.clone();
            let registry = host.host_context().async_resource_registry_ref().unwrap();
            let tx = old_tx.clone();
            tokio::spawn(async move {
                let result = if create {
                    registry
                        .create_capability_resource(
                            "changing".into(),
                            format!("old-{index}"),
                            "cap.v1".into(),
                        )
                        .await
                        .map(|_| ())
                } else {
                    client
                        .collect_read_plan(factory_read("changing"))
                        .await
                        .map(|_| ())
                };
                tx.send(result).unwrap();
            });
            started.recv().await.unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let mut candidate = RuntimeBootstrapper::new();
        candidate.register_manifest(manifest);
        candidate.register_async_resource_provider(
            "changing",
            Arc::new(FactoryAsyncProvider {
                calls: calls.clone(),
                panic: false,
                pause: None,
            }),
        );
        host.reload(
            candidate.prepare_reload(runtime_profile(), 2).unwrap(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(weak.upgrade().is_some());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..2 {
            let client = gateway.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                tx.send(client.collect_read_plan(factory_read("changing")).await)
                    .unwrap();
            });
        }
        let rejected = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        // Release both old requests even when the regression assertions will fail.
        release.add_permits(1);
        old_rx.recv().await.unwrap().unwrap();
        host.statistics().unwrap();
        let after_first = calls.load(Ordering::SeqCst);
        let retained_after_first = weak.upgrade().is_some();
        release.add_permits(1);
        old_rx.recv().await.unwrap().unwrap();
        let final_result = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        assert_eq!(
            rejected.unwrap().unwrap().unwrap_err().error().code,
            ERR_CAPABILITY_EXHAUSTED
        );
        assert_eq!(after_first, 0);
        assert!(retained_after_first);
        final_result.unwrap().unwrap().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(weak.upgrade().is_none());
        if create {
            for index in 0..2 {
                host.host_context()
                    .async_resource_registry()
                    .unwrap()
                    .open_resource_descriptor(format!("changing:old-{index}"))
                    .await
                    .unwrap();
            }
        }
    });
}

type CreatePause = (
    std::sync::mpsc::Sender<()>,
    std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
);

#[derive(Clone)]
struct CreateProvider {
    calls: Arc<AtomicUsize>,
    pause: Option<Arc<CreatePause>>,
    execution: mutsuki_runtime_sdk::ResourceProviderExecution,
}
impl CreateProvider {
    fn outcome(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderOutcome<mutsuki_runtime_sdk::ResourceProviderReply>
    {
        use mutsuki_runtime_sdk::{
            ResourceProviderOutcome as O, ResourceProviderReply as R, ResourceProviderRequest as Q,
        };
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0
            && let Some(pause) = &self.pause
        {
            pause.0.send(()).unwrap();
            pause
                .1
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(15))
                .unwrap();
        }
        let (kind, schema) = match request {
            Q::CreateBlob { schema, .. } => ("blob".to_string(), schema),
            Q::CreateCow {
                kind_id, schema, ..
            }
            | Q::CreateCapability { kind_id, schema } => (kind_id, schema),
            Q::Collect(_) => return O::new(Ok(R::Bytes(b"bridge".to_vec()))),
            _ => panic!("unexpected request"),
        };
        if schema == "panic" {
            panic!("create panic");
        }
        if schema == "fail" {
            return O::new(Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_UNSUPPORTED,
                "test.create",
                "failed",
            ))));
        }
        let descriptor = FakeAsyncProvider {
            provider_id: if schema == "wrong-owner" {
                "other"
            } else {
                "create"
            }
            .into(),
            calls: self.calls.clone(),
        }
        .resource(&kind, &schema);
        O::new(Ok(R::Created(descriptor)))
    }
}
impl mutsuki_runtime_sdk::ResourceProviderGateway for CreateProvider {
    fn execution(&self) -> mutsuki_runtime_sdk::ResourceProviderExecution {
        self.execution
    }
    fn ordering(&self) -> mutsuki_runtime_sdk::ResourceProviderOrdering {
        mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered
    }
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderOutcome<mutsuki_runtime_sdk::ResourceProviderReply>
    {
        self.outcome(request)
    }
}
impl AsyncResourceProviderGateway for CreateProvider {
    fn ordering(&self) -> mutsuki_runtime_sdk::ResourceProviderOrdering {
        mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered
    }
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderFuture {
        // Deliberately do the work before returning the Future: construction must
        // be admitted, isolated, and off the actor too.
        let outcome = self.outcome(request);
        Box::pin(async move { outcome })
    }
}
fn create_host(
    provider: CreateProvider,
    native: bool,
    config: HostRuntimeConfig,
) -> crate::HostRuntime {
    let mut bootstrap = super::helpers::host_with_echo_runner();
    let mut manifest = runner_manifest("create-plugin", vec![]);
    manifest.provides.resource_providers = vec!["create".into(), "other".into()];
    bootstrap.register_manifest(manifest);
    let mut other = create_provider();
    other.execution = mutsuki_runtime_sdk::ResourceProviderExecution::Inline;
    bootstrap.register_resource_provider("other", Arc::new(other));
    if native {
        bootstrap.register_async_resource_provider("create", Arc::new(provider));
    } else {
        bootstrap.register_resource_provider("create", Arc::new(provider));
    }
    let mut profile = runtime_profile();
    profile.enabled_plugins.push("create-plugin".into());
    bootstrap
        .into_host_runtime_with_config(profile, config)
        .unwrap()
}
fn create_config(bytes: usize) -> HostRuntimeConfig {
    HostRuntimeConfig {
        event_driven: true,
        ..HostRuntimeConfig::default()
    }
    .with_async_executor(Arc::new(TokioAsyncExecutor::new(2, 8, 8, bytes).unwrap()))
}
fn create_provider() -> CreateProvider {
    CreateProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        pause: None,
        execution: mutsuki_runtime_sdk::ResourceProviderExecution::Offloaded,
    }
}
fn async_test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn async_registry_registers_all_create_kinds_and_rejects_invalid_results() {
    for mode in 0..3 {
        let mut provider = create_provider();
        if mode == 2 {
            provider.execution = mutsuki_runtime_sdk::ResourceProviderExecution::Inline;
        }
        let host = create_host(provider, mode == 0, create_config(1024 * 1024));
        let registry = host.host_context().async_resource_registry_ref().unwrap();
        async_test_runtime().block_on(async {
            let descriptors = [
                registry
                    .create_blob_resource("create".into(), "blob.v1".into(), vec![42])
                    .await
                    .unwrap(),
                registry
                    .create_cow_state_resource(
                        "create".into(),
                        "cow".into(),
                        "cow.v1".into(),
                        vec![43],
                    )
                    .await
                    .unwrap(),
                registry
                    .create_capability_resource("create".into(), "cap".into(), "cap.v1".into())
                    .await
                    .unwrap(),
            ];
            for descriptor in &descriptors {
                assert_eq!(
                    &registry
                        .open_resource_descriptor(descriptor.ref_id.to_string())
                        .await
                        .unwrap(),
                    descriptor
                );
            }
            assert!(
                registry
                    .create_blob_resource("create".into(), "duplicate".into(), vec![])
                    .await
                    .is_err()
            );
            assert!(
                registry
                    .create_capability_resource("create".into(), "bad".into(), "wrong-owner".into())
                    .await
                    .is_err()
            );
            assert!(
                registry
                    .create_capability_resource("create".into(), "failed".into(), "fail".into())
                    .await
                    .is_err()
            );
            assert_eq!(
                registry
                    .open_resource_descriptor("other:bad".into())
                    .await
                    .unwrap_err()
                    .error()
                    .code,
                ERR_RESOURCE_NOT_FOUND
            );
            assert_eq!(
                registry
                    .open_resource_descriptor("create:failed".into())
                    .await
                    .unwrap_err()
                    .error()
                    .code,
                ERR_RESOURCE_NOT_FOUND
            );
            assert_eq!(
                registry
                    .open_resource_descriptor(descriptors[0].ref_id.to_string())
                    .await
                    .unwrap(),
                descriptors[0]
            );
        });
    }
}

#[test]
fn create_admission_rejects_bytes_without_calling_provider_then_recovers() {
    for native in [false, true] {
        let provider = create_provider();
        let calls = provider.calls.clone();
        let host = create_host(provider, native, create_config(128));
        let registry = host.host_context().async_resource_registry_ref().unwrap();
        async_test_runtime().block_on(async {
            let rejected = registry
                .create_blob_resource("create".into(), "blob.v1".into(), vec![0; 129])
                .await;
            assert_eq!(rejected.unwrap_err().error().code, ERR_CAPABILITY_EXHAUSTED);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            registry
                .create_blob_resource("create".into(), "blob.v1".into(), vec![42])
                .await
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        });
    }
}

#[test]
fn async_create_and_future_construction_leave_actor_responsive() {
    for native in [false, true] {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut provider = create_provider();
        provider.pause = Some(Arc::new((entered_tx, std::sync::Mutex::new(release_rx))));
        let host = create_host(provider, native, create_config(16 * 1024 * 1024));
        let registry = host.host_context().async_resource_registry_ref().unwrap();
        let pending = std::thread::spawn(move || {
            async_test_runtime().block_on(registry.create_blob_resource(
                "create".into(),
                "blob.v1".into(),
                vec![42; 8 * 1024 * 1024],
            ))
        });
        let entered = entered_rx.recv_timeout(Duration::from_secs(10));
        let context = host.host_context().clone();
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        let probe = std::thread::spawn(move || {
            let result = async_test_runtime().block_on(
                context
                    .async_resource_registry()
                    .unwrap()
                    .open_resource_descriptor("absent".into()),
            );
            let unrelated = async_test_runtime().block_on(
                context
                    .async_resource_gateway()
                    .unwrap()
                    .collect_read_plan(factory_read("other")),
            );
            let submitted = context
                .task_submitter()
                .submit_one(Task::new("during-create", "raw.input", json!({})))
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let completed = loop {
                if let Some(outcome) = context.task_submitter().task_outcome(&submitted).unwrap() {
                    break Some(outcome);
                }
                if std::time::Instant::now() >= deadline {
                    break None;
                }
                std::thread::yield_now();
            };
            let _ = probe_tx.send((result, completed, unrelated));
        });
        let probed = probe_rx.recv_timeout(Duration::from_secs(10));
        // Release before assertions or Host teardown, even when the actor regresses.
        let _ = release_tx.send(());
        let descriptor = pending.join().unwrap().unwrap();
        probe.join().unwrap();
        entered.unwrap();
        let (opened, completed, unrelated) = probed.unwrap();
        assert_eq!(unrelated.unwrap(), b"bridge");
        assert_eq!(opened.unwrap_err().error().code, ERR_RESOURCE_NOT_FOUND);
        assert!(matches!(completed, Some(TaskOutcome::Completed { .. })));
        assert_eq!(
            host.host_context()
                .resource_registry()
                .open_resource_descriptor(descriptor.ref_id.as_str())
                .unwrap(),
            descriptor
        );
        assert_eq!(
            host.host_context()
                .resource_gateway()
                .collect_read_plan(&ReadPlan {
                    plan_id: "bridged".into(),
                    resource: descriptor,
                    operation: "collect".into(),
                    args: json!({})
                })
                .unwrap(),
            b"bridge"
        );
    }
}

#[test]
fn create_panic_poison_and_missing_executor_fail_without_inline_fallback() {
    for native in [false, true] {
        let provider = create_provider();
        let calls = provider.calls.clone();
        let host = create_host(provider, native, HostRuntimeConfig::default());
        let registry = host.host_context().async_resource_registry_ref().unwrap();
        async_test_runtime().block_on(async {
            assert!(
                registry
                    .create_blob_resource("create".into(), "blob.v1".into(), vec![])
                    .await
                    .is_err()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                registry
                    .open_resource_descriptor("absent".into())
                    .await
                    .unwrap_err()
                    .error()
                    .code,
                ERR_RESOURCE_NOT_FOUND
            );
        });
        let host = create_host(create_provider(), native, create_config(1024));
        let registry = host.host_context().async_resource_registry_ref().unwrap();
        async_test_runtime().block_on(async {
            assert_eq!(
                registry
                    .create_blob_resource("create".into(), "panic".into(), vec![])
                    .await
                    .unwrap_err()
                    .error()
                    .code,
                ERR_RUNTIME_HOST_FAILED
            );
            assert_eq!(
                registry
                    .create_blob_resource("create".into(), "blob.v1".into(), vec![])
                    .await
                    .unwrap_err()
                    .error()
                    .code,
                ERR_RUNTIME_HOST_FAILED
            );
        });
        host.statistics().unwrap();
    }
}

struct DeferredCreateProvider {
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Semaphore>,
    calls: Arc<AtomicUsize>,
}
impl AsyncResourceProviderGateway for DeferredCreateProvider {
    fn ordering(&self) -> mutsuki_runtime_sdk::ResourceProviderOrdering {
        mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered
    }
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderFuture {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        let entered = self.entered.clone();
        let release = self.release.clone();
        Box::pin(async move {
            if first {
                let _ = entered.send(());
                release.acquire().await.unwrap().forget();
            }
            create_provider().outcome(request)
        })
    }
}

#[test]
fn timed_out_or_disconnected_create_applies_before_next_ordered_reply() {
    for disconnect in [false, true] {
        let (entered, mut started) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut bootstrap = RuntimeBootstrapper::new();
        let mut manifest = runner_manifest("plugin-a", vec![]);
        manifest.provides.resource_providers = vec!["create".into()];
        bootstrap.register_manifest(manifest);
        bootstrap.register_async_resource_provider(
            "create",
            Arc::new(DeferredCreateProvider {
                entered,
                release: release.clone(),
                calls: calls.clone(),
            }),
        );
        let mut config = create_config(1024);
        config.default_runner_limits.wall_clock_deadline = Some(Duration::from_millis(100));
        config.actor_data_queue_limit = 1;
        let host = bootstrap
            .into_host_runtime_with_config(runtime_profile(), config)
            .unwrap();
        let registry = host.host_context().async_resource_registry_ref().unwrap();
        async_test_runtime().block_on(async {
            let client = registry.clone();
            let first = tokio::spawn(async move {
                client
                    .create_blob_resource("create".into(), "blob.v1".into(), vec![42])
                    .await
            });
            started.recv().await.unwrap();
            if disconnect {
                first.abort();
                let _ = first.await;
            } else {
                let timed_out = tokio::time::timeout(Duration::from_secs(2), first).await;
                if timed_out.is_err() {
                    release.add_permits(1);
                }
                assert!(timed_out.unwrap().unwrap().is_err());
            }
            let before = registry
                .open_resource_descriptor("create:blob".into())
                .await;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            for kind in ["next-a", "next-b"] {
                let client = registry.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(
                        client
                            .create_capability_resource(
                                "create".into(),
                                kind.into(),
                                "cap.v1".into(),
                            )
                            .await,
                    );
                });
            }
            let rejected = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
            let before_calls = calls.load(Ordering::SeqCst);
            release.add_permits(1);
            let next = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(before.unwrap_err().error().code, ERR_RESOURCE_NOT_FOUND);
            assert_eq!(before_calls, 1);
            assert_eq!(
                rejected.unwrap().unwrap().unwrap_err().error().code,
                ERR_CAPABILITY_EXHAUSTED
            );
            assert_eq!(
                registry
                    .open_resource_descriptor(next.ref_id.to_string())
                    .await
                    .unwrap(),
                next
            );
            registry
                .open_resource_descriptor("create:blob".into())
                .await
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        });
    }
}
