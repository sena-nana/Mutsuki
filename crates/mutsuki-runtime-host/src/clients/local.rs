use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SnapshotDescriptor, StreamPlan, TaskBatch,
    TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};
use mutsuki_runtime_sdk::{
    ResourcePlanGateway, ResourceProviderGateway, ResourceProviderReply as Reply,
    ResourceProviderRequest as Request, TaskSubmitter,
};

use crate::error::{resource_provider_missing, resource_provider_unsupported};
use crate::resource_router::single_command_provider;

#[derive(Clone)]
pub struct LocalTaskClient {
    runtime: Arc<Mutex<CoreRuntime>>,
}

impl LocalTaskClient {
    pub fn new(runtime: Arc<Mutex<CoreRuntime>>) -> Self {
        Self { runtime }
    }
}

impl TaskSubmitter for LocalTaskClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .submit_batch(batch)
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .cancel_task_handle(handle)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .task_handle_outcome(handle)
    }
}

#[derive(Clone)]
pub struct LocalResourceClient {
    providers: BTreeMap<String, Arc<dyn ResourceProviderGateway>>,
}

impl LocalResourceClient {
    pub fn with_provider(
        provider_id: impl Into<String>,
        provider: impl ResourceProviderGateway + 'static,
    ) -> Self {
        Self::from_provider(provider_id, Arc::new(provider))
    }

    pub fn from_provider(
        provider_id: impl Into<String>,
        provider: Arc<dyn ResourceProviderGateway>,
    ) -> Self {
        Self {
            providers: BTreeMap::from([(provider_id.into(), provider)]),
        }
    }

    pub fn with_providers<I>(providers: I) -> Self
    where
        I: IntoIterator<Item = (String, Arc<dyn ResourceProviderGateway>)>,
    {
        Self {
            providers: providers.into_iter().collect(),
        }
    }

    fn require_provider(&self, provider_id: &str) -> RuntimeResult<&dyn ResourceProviderGateway> {
        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| resource_provider_missing(provider_id))?;
        if provider.ordering() == mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered {
            return Err(resource_provider_unsupported(
                "lifecycle providers require the Host actor resource client",
            ));
        }
        Ok(provider.as_ref())
    }

    fn execute(&self, provider_id: &str, request: Request) -> RuntimeResult<Reply> {
        self.require_provider(provider_id)?.execute(request).result
    }

    fn receipt(&self, provider_id: &str, request: Request) -> RuntimeResult<PlanReceipt> {
        match self.execute(provider_id, request)? {
            Reply::Receipt(value) => Ok(*value),
            _ => Err(resource_provider_unsupported(
                "provider reply type mismatch",
            )),
        }
    }

    fn receipts(&self, provider_id: &str, request: Request) -> RuntimeResult<Vec<PlanReceipt>> {
        match self.execute(provider_id, request)? {
            Reply::Receipts(value) => Ok(value),
            _ => Err(resource_provider_unsupported(
                "provider reply type mismatch",
            )),
        }
    }
}

impl ResourcePlanGateway for LocalResourceClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        match self.execute(&plan.resource.provider_id, Request::Collect(plan.clone()))? {
            Reply::Bytes(value) => Ok(value),
            _ => Err(resource_provider_unsupported(
                "provider reply type mismatch",
            )),
        }
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        match self.execute(
            &plan.resource.provider_id,
            Request::Snapshot {
                plan: plan.clone(),
                kind_id: kind_id.into(),
                schema: schema.into(),
            },
        )? {
            Reply::Snapshot(value) => Ok(*value),
            _ => Err(resource_provider_unsupported(
                "provider reply type mismatch",
            )),
        }
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        match self.execute(
            &plan.resource.provider_id,
            Request::OpenStream(plan.clone()),
        )? {
            Reply::Stream(value) => Ok(value),
            _ => Err(resource_provider_unsupported(
                "provider reply type mismatch",
            )),
        }
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.receipt(&plan.resource.provider_id, Request::Export(plan.clone()))
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.receipt(
            &plan.resource.provider_id,
            Request::Commit {
                plan: Box::new(plan.clone()),
                bytes,
            },
        )
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.receipt(&plan.capability.provider_id, Request::Command(plan.clone()))
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.receipts(
            &single_command_provider(batch.commands.iter())?,
            Request::Batch(batch.clone()),
        )
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.receipts(
            &single_command_provider(saga.steps.iter().chain(saga.compensations.iter()))?,
            Request::Saga(saga.clone()),
        )
    }
}
