//! Host services handed out before the runtime that serves them exists.

use std::sync::{Arc, OnceLock};
use std::task::Waker;
use std::time::Duration;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RuntimeError, SnapshotDescriptor,
    StreamPlan, TaskBatch, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_sdk::{
    HostContext, ResourcePlanGateway, ResourceRegistryGateway, RuntimeClient, RuntimeFailure,
    RuntimeResult, TaskSubmitter,
};

struct BoundServices {
    tasks: Arc<dyn TaskSubmitter>,
    resources: Arc<dyn ResourcePlanGateway>,
    registry: Arc<dyn ResourceRegistryGateway>,
}

/// Task and resource services for components built before their `HostRuntime` boots.
///
/// Runners, ABI plugins and embedded engines need a client at construction time, while the actor
/// that serves it only exists once boot finishes. The embedder binds this exactly once from the
/// booted runtime's [`HostContext`]; until then every call fails with
/// `runtime_client.not_bound`. All calls go through the actor gateways, never around them.
pub struct DeferredHostClient {
    source: &'static str,
    services: OnceLock<BoundServices>,
}

impl DeferredHostClient {
    /// `source` names the embedder in structured errors, e.g. `"mutsuki.service.runtime"`.
    pub fn new(source: &'static str) -> Self {
        Self {
            source,
            services: OnceLock::new(),
        }
    }

    pub fn bind(&self, context: &HostContext) -> RuntimeResult<()> {
        self.services
            .set(BoundServices {
                tasks: context.task_submitter_ref(),
                resources: context.resource_gateway_ref(),
                registry: context.resource_registry_ref(),
            })
            .map_err(|_| self.failure("runtime_client.already_bound"))
    }

    fn services(&self) -> RuntimeResult<&BoundServices> {
        self.services
            .get()
            .ok_or_else(|| self.failure("runtime_client.not_bound"))
    }

    fn failure(&self, route: &str) -> RuntimeFailure {
        RuntimeFailure::new(RuntimeError::new(
            mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
            self.source,
            route,
        ))
    }
}

impl RuntimeClient for DeferredHostClient {}

impl TaskSubmitter for DeferredHostClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.services()?.tasks.submit_batch(batch)
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.services()?.tasks.cancel_task(handle)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.services()?.tasks.task_outcome(handle)
    }

    fn wait_outcome(
        &self,
        handle: &TaskHandle,
        timeout: Option<Duration>,
    ) -> RuntimeResult<Option<TaskOutcome>> {
        self.services()?.tasks.wait_outcome(handle, timeout)
    }

    /// Before binding there is no actor to park the waker with; waking now lets the poller
    /// observe `runtime_client.not_bound` instead of hanging.
    fn register_waker(&self, handle: &TaskHandle, waker: &Waker) {
        match self.services() {
            Ok(services) => services.tasks.register_waker(handle, waker),
            Err(_) => waker.wake_by_ref(),
        }
    }
}

impl ResourcePlanGateway for DeferredHostClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.services()?.resources.collect_read_plan(plan)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.services()?
            .resources
            .snapshot_read_plan(plan, kind_id, schema)
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.services()?.resources.open_stream_plan(plan)
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.services()?.resources.execute_export_plan(plan)
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.services()?.resources.commit_write_plan(plan, bytes)
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.services()?.resources.execute_command_plan(plan)
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.services()?.resources.execute_command_batch(batch)
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.services()?.resources.execute_saga_plan(saga)
    }
}

impl ResourceRegistryGateway for DeferredHostClient {
    fn open_resource_descriptor(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.services()?.registry.open_resource_descriptor(ref_id)
    }

    fn create_blob_resource(
        &self,
        provider_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.services()?
            .registry
            .create_blob_resource(provider_id, schema, bytes)
    }

    fn create_cow_state_resource(
        &self,
        provider_id: &str,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.services()?
            .registry
            .create_cow_state_resource(provider_id, kind_id, schema, bytes)
    }

    fn create_capability_resource(
        &self,
        provider_id: &str,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        self.services()?
            .registry
            .create_capability_resource(provider_id, kind_id, schema)
    }
}
