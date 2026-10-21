//! Runtime client handed to components before the Host runtime exists; it
//! binds to the actor-backed gateways once boot completes.

#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Default)]
pub(crate) struct DeferredRuntimeClient {
    pub(crate) runtime: OnceLock<RuntimeClientRef>,
    pub(crate) task_submitter: OnceLock<Arc<dyn TaskSubmitter>>,
    pub(crate) resource_gateway: OnceLock<Arc<dyn ResourcePlanGateway>>,
    pub(crate) resource_registry: OnceLock<Arc<dyn ResourceRegistryGateway>>,
}

impl DeferredRuntimeClient {
    pub(crate) fn bind(
        &self,
        task_submitter: Arc<dyn TaskSubmitter>,
        resource_gateway: Arc<dyn ResourcePlanGateway>,
        resource_registry: Arc<dyn ResourceRegistryGateway>,
    ) {
        let runtime = TaskSubmitterRuntimeClient::new(task_submitter.clone()).into_runtime_client();
        assert!(
            self.runtime.set(runtime).is_ok(),
            "runtime client already bound"
        );
        assert!(
            self.task_submitter.set(task_submitter).is_ok(),
            "task submitter already bound"
        );
        assert!(
            self.resource_gateway.set(resource_gateway).is_ok(),
            "resource gateway already bound"
        );
        assert!(
            self.resource_registry.set(resource_registry).is_ok(),
            "resource registry already bound"
        );
    }

    pub(crate) fn client(&self) -> RuntimeResult<RuntimeClientRef> {
        self.runtime.get().cloned().ok_or_else(|| {
            RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "mutsuki.service.runtime",
                "runtime_client.not_bound",
            ))
        })
    }

    pub(crate) fn task_submitter(&self) -> RuntimeResult<Arc<dyn TaskSubmitter>> {
        self.task_submitter
            .get()
            .cloned()
            .ok_or_else(deferred_not_bound)
    }

    pub(crate) fn resource_gateway(&self) -> RuntimeResult<Arc<dyn ResourcePlanGateway>> {
        self.resource_gateway
            .get()
            .cloned()
            .ok_or_else(deferred_not_bound)
    }

    pub(crate) fn resource_registry(&self) -> RuntimeResult<Arc<dyn ResourceRegistryGateway>> {
        self.resource_registry
            .get()
            .cloned()
            .ok_or_else(deferred_not_bound)
    }
}

pub(crate) fn deferred_not_bound() -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "mutsuki.service.runtime",
        "runtime_client.not_bound",
    ))
}

impl RuntimeClient for DeferredRuntimeClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.client()?.submit_batch(batch)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.client()?.task_outcome(handle)
    }

    fn register_waker(&self, handle: &TaskHandle, waker: &Waker) {
        if let Ok(client) = self.client() {
            client.register_waker(handle, waker);
        }
    }

    fn wait_outcome(
        &self,
        handle: &TaskHandle,
        timeout: Option<Duration>,
    ) -> RuntimeResult<Option<TaskOutcome>> {
        self.client()?.wait_outcome(handle, timeout)
    }
}

impl TaskSubmitter for DeferredRuntimeClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.task_submitter()?.submit_batch(batch)
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.task_submitter()?.cancel_task(handle)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.task_submitter()?.task_outcome(handle)
    }

    fn wait_outcome(
        &self,
        handle: &TaskHandle,
        timeout: Option<Duration>,
    ) -> RuntimeResult<Option<TaskOutcome>> {
        self.task_submitter()?.wait_outcome(handle, timeout)
    }
}

impl ResourcePlanGateway for DeferredRuntimeClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.resource_gateway()?.collect_read_plan(plan)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.resource_gateway()?
            .snapshot_read_plan(plan, kind_id, schema)
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.resource_gateway()?.open_stream_plan(plan)
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.resource_gateway()?.execute_export_plan(plan)
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.resource_gateway()?.commit_write_plan(plan, bytes)
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.resource_gateway()?.execute_command_plan(plan)
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.resource_gateway()?.execute_command_batch(batch)
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.resource_gateway()?.execute_saga_plan(saga)
    }
}

impl ResourceRegistryGateway for DeferredRuntimeClient {
    fn open_resource_descriptor(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.resource_registry()?.open_resource_descriptor(ref_id)
    }

    fn create_blob_resource(
        &self,
        provider_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.resource_registry()?
            .create_blob_resource(provider_id, schema, bytes)
    }

    fn create_cow_state_resource(
        &self,
        provider_id: &str,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.resource_registry()?
            .create_cow_state_resource(provider_id, kind_id, schema, bytes)
    }

    fn create_capability_resource(
        &self,
        provider_id: &str,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        self.resource_registry()?
            .create_capability_resource(provider_id, kind_id, schema)
    }
}
