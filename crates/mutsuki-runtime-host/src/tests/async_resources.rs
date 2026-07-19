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

impl FakeAsyncProvider {
    fn resource(&self, kind_id: &str, schema: &str) -> ResourceRef {
        ResourceRef {
            ref_id: format!("{}:{kind_id}", self.provider_id),
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

impl AsyncResourceProviderGateway for FakeAsyncProvider {
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
fn fake_db_http_and_link_providers_execute_only_through_async_gateway() {
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

    let sync_error = runtime
        .host_context()
        .resource_gateway()
        .execute_command_plan(&plans[0])
        .expect_err("async provider must not block the synchronous gateway");
    assert_eq!(sync_error.error().code, ERR_REGISTRY_UNAUTHORIZED);

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
        let (db, http, link) = futures_util::future::join3(db, http, link).await;
        vec![db.unwrap(), http.unwrap(), link.unwrap()]
    });

    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        receipts
            .iter()
            .map(|receipt| receipt.output["provider"].as_str().unwrap())
            .collect::<Vec<_>>(),
        provider_ids
    );
}
