//! Compatibility adapter from the Core runner/plugin model to the independent ABI host.

use std::path::PathBuf;
use std::sync::Arc;

use mutsuki_plugin_api::{
    PluginHostContext, PluginHostError, PluginResourceGateway, PluginTaskGateway,
};
use mutsuki_plugin_host::{PluginLoadRequest, PluginSession};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, SnapshotDescriptor, StreamPlan,
    TaskBatch, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    HostEffect, HostEffectFuture, HostEffectKind, LoadedPlugin, ResourcePlanGateway,
    RuntimeBootstrapperEffect, RuntimeBootstrapperResourceProvider, TaskSubmitter,
};
use mutsuki_runtime_wire::WireRequest;
use serde_json::Value;

use crate::{TransportResourceProvider, TransportRunner, TypedRequestTransport};

/// Inputs required to connect an already discovered and validated ABI v2 artifact.
///
/// Package discovery, archive extraction, path/hash validation, cache selection and reload
/// generation ownership stay with the product Host.
pub struct AbiPluginLoadRequest {
    pub library_path: PathBuf,
    pub expected_manifest: mutsuki_runtime_contracts::PluginManifest,
    pub config: Option<Value>,
    pub task_submitter: Arc<dyn TaskSubmitter>,
    pub resource_gateway: Arc<dyn ResourcePlanGateway>,
}

/// Loads an ABI v2 plugin through the Core-independent host and exposes the old `LoadedPlugin`
/// surface to existing ServiceHost/TauriHost callers.
pub fn load_abi_plugin_v2(request: AbiPluginLoadRequest) -> RuntimeResult<LoadedPlugin> {
    let host_context = PluginHostContext::default()
        .with_task_gateway(Arc::new(TaskGatewayAdapter {
            inner: request.task_submitter,
        }))
        .with_resource_gateway(Arc::new(ResourceGatewayAdapter {
            inner: request.resource_gateway,
        }));
    let session = Arc::new(
        PluginSession::load(PluginLoadRequest {
            library_path: request.library_path,
            expected_manifest: request.expected_manifest,
            config: request.config,
            host_context,
            host_config: Default::default(),
        })
        .map_err(to_runtime_failure)?,
    );
    let transport = Arc::new(SessionTransport(session.clone()));
    let runners = session
        .runners()
        .map(|runner| {
            Box::new(TransportRunner::new(
                runner.descriptor().clone(),
                transport.clone(),
            )) as Box<dyn mutsuki_runtime_core::Runner>
        })
        .collect();
    let resource_providers = session
        .initialized()
        .resource_provider_ids
        .iter()
        .cloned()
        .map(|provider_id| RuntimeBootstrapperResourceProvider {
            provider_id: provider_id.clone(),
            provider: Arc::new(TransportResourceProvider::new(
                provider_id,
                transport.clone(),
            )),
        })
        .collect();

    Ok(LoadedPlugin {
        manifest: session.manifest().clone(),
        runners,
        async_handlers: Vec::new(),
        host_services: Vec::new(),
        resource_providers,
        async_resource_providers: Vec::new(),
        host_effects: vec![RuntimeBootstrapperEffect {
            kind: HostEffectKind::BackendInstance,
            effect: Box::new(AbiSessionEffect(session)),
        }],
    })
}

struct AbiSessionEffect(Arc<PluginSession>);

impl HostEffect for AbiSessionEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        Box::pin(async move { self.0.dispose().map_err(to_runtime_failure) })
    }
}

struct SessionTransport(Arc<PluginSession>);

impl TypedRequestTransport for SessionTransport {
    fn request<R: WireRequest>(&self, request: &R) -> RuntimeResult<R::Response> {
        self.0.request(request).map_err(to_runtime_failure)
    }
}

struct TaskGatewayAdapter {
    inner: Arc<dyn TaskSubmitter>,
}

impl PluginTaskGateway for TaskGatewayAdapter {
    fn submit_batch(&self, batch: TaskBatch) -> mutsuki_plugin_api::PluginResult<Vec<TaskHandle>> {
        self.inner.submit_batch(batch).map_err(to_plugin_error)
    }

    fn cancel_task(&self, handle: &TaskHandle) -> mutsuki_plugin_api::PluginResult<()> {
        self.inner.cancel_task(handle).map_err(to_plugin_error)
    }

    fn task_outcome(
        &self,
        handle: &TaskHandle,
    ) -> mutsuki_plugin_api::PluginResult<Option<TaskOutcome>> {
        self.inner.task_outcome(handle).map_err(to_plugin_error)
    }
}

struct ResourceGatewayAdapter {
    inner: Arc<dyn ResourcePlanGateway>,
}

impl PluginResourceGateway for ResourceGatewayAdapter {
    fn collect_read_plan(
        &self,
        _provider_id: Option<&str>,
        plan: &ReadPlan,
    ) -> mutsuki_plugin_api::PluginResult<Vec<u8>> {
        self.inner.collect_read_plan(plan).map_err(to_plugin_error)
    }

    fn snapshot_read_plan(
        &self,
        _provider_id: Option<&str>,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> mutsuki_plugin_api::PluginResult<SnapshotDescriptor> {
        self.inner
            .snapshot_read_plan(plan, kind_id, schema)
            .map_err(to_plugin_error)
    }

    fn open_stream_plan(
        &self,
        _provider_id: Option<&str>,
        plan: &ReadPlan,
    ) -> mutsuki_plugin_api::PluginResult<StreamPlan> {
        self.inner.open_stream_plan(plan).map_err(to_plugin_error)
    }

    fn execute_export_plan(
        &self,
        _provider_id: Option<&str>,
        plan: &ExportPlan,
    ) -> mutsuki_plugin_api::PluginResult<PlanReceipt> {
        self.inner
            .execute_export_plan(plan)
            .map_err(to_plugin_error)
    }

    fn commit_write_plan(
        &self,
        _provider_id: Option<&str>,
        plan: &WritePlan,
        bytes: Vec<u8>,
    ) -> mutsuki_plugin_api::PluginResult<PlanReceipt> {
        self.inner
            .commit_write_plan(plan, bytes)
            .map_err(to_plugin_error)
    }

    fn execute_command_plan(
        &self,
        _provider_id: Option<&str>,
        plan: &CommandPlan,
    ) -> mutsuki_plugin_api::PluginResult<PlanReceipt> {
        self.inner
            .execute_command_plan(plan)
            .map_err(to_plugin_error)
    }

    fn execute_command_batch(
        &self,
        _provider_id: Option<&str>,
        batch: &mutsuki_runtime_contracts::resource::experimental::CommandBatch,
    ) -> mutsuki_plugin_api::PluginResult<Vec<PlanReceipt>> {
        self.inner
            .execute_command_batch(batch)
            .map_err(to_plugin_error)
    }

    fn execute_saga_plan(
        &self,
        _provider_id: Option<&str>,
        saga: &mutsuki_runtime_contracts::resource::experimental::SagaPlan,
    ) -> mutsuki_plugin_api::PluginResult<Vec<PlanReceipt>> {
        self.inner.execute_saga_plan(saga).map_err(to_plugin_error)
    }

    fn create_blob_resource(
        &self,
        _provider_id: Option<&str>,
        _schema: &str,
        _bytes: Vec<u8>,
    ) -> mutsuki_plugin_api::PluginResult<ResourceRef> {
        Err(capability_unavailable("resource.create_blob"))
    }

    fn create_cow_state_resource(
        &self,
        _provider_id: Option<&str>,
        _kind_id: &str,
        _schema: &str,
        _bytes: Vec<u8>,
    ) -> mutsuki_plugin_api::PluginResult<ResourceRef> {
        Err(capability_unavailable("resource.create_cow_state"))
    }

    fn create_capability_resource(
        &self,
        _provider_id: Option<&str>,
        _kind_id: &str,
        _schema: &str,
    ) -> mutsuki_plugin_api::PluginResult<ResourceRef> {
        Err(capability_unavailable("resource.create_capability"))
    }
}

fn to_plugin_error(error: RuntimeFailure) -> PluginHostError {
    PluginHostError {
        error: error.error().clone(),
    }
}

fn to_runtime_failure(error: PluginHostError) -> RuntimeFailure {
    RuntimeFailure::new(error.into_runtime_error())
}

fn capability_unavailable(capability: &str) -> PluginHostError {
    let mut error = PluginHostError::new(
        "plugin.capability_unavailable",
        "mutsuki.runtime-host",
        "plugin.capability_unavailable",
        format!("host capability is unavailable: {capability}"),
    );
    error.error.lost_capability = Some(capability.to_string());
    error.error.recovery = Some("provide a ResourceProviderGateway adapter".into());
    error
}
