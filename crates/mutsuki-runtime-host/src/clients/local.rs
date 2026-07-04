use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SagaPlan, SnapshotDescriptor,
    StreamPlan, Task, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};
use mutsuki_runtime_sdk::{ResourcePlanGateway, TaskSubmitter};

use super::ResourcePlanProvider;
use crate::error::{resource_provider_missing, resource_provider_unsupported};

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
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .submit_task_handle(task)
    }

    fn cancel_task(&self, task_id: &str) -> RuntimeResult<()> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .cancel_task(task_id)
    }

    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .task_outcome(task_id)
    }
}

#[derive(Clone)]
pub struct LocalResourceClient {
    providers: BTreeMap<String, Arc<dyn ResourcePlanProvider>>,
}

impl LocalResourceClient {
    pub fn with_provider(
        provider_id: impl Into<String>,
        provider: impl ResourcePlanProvider + 'static,
    ) -> Self {
        Self::from_provider(provider_id, Arc::new(provider))
    }

    pub fn from_provider(
        provider_id: impl Into<String>,
        provider: Arc<dyn ResourcePlanProvider>,
    ) -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(provider_id.into(), provider);
        Self { providers }
    }

    pub fn with_providers<I>(providers: I) -> Self
    where
        I: IntoIterator<Item = (String, Arc<dyn ResourcePlanProvider>)>,
    {
        Self {
            providers: providers.into_iter().collect(),
        }
    }

    fn require_provider(&self, provider_id: &str) -> RuntimeResult<&dyn ResourcePlanProvider> {
        self.providers
            .get(provider_id)
            .map(|provider| provider.as_ref())
            .ok_or_else(|| resource_provider_missing(provider_id))
    }

    fn single_command_provider<'a>(
        commands: impl Iterator<Item = &'a CommandPlan>,
    ) -> RuntimeResult<String> {
        let mut provider_id = None;
        for command in commands {
            match provider_id {
                Some(existing) if existing != command.capability.provider_id => {
                    return Err(resource_provider_unsupported(
                        "command collection spans multiple resource providers",
                    ));
                }
                Some(_) => {}
                None => provider_id = Some(command.capability.provider_id.clone()),
            }
        }
        provider_id.ok_or_else(|| {
            resource_provider_unsupported("command collection has no provider route")
        })
    }
}

impl ResourcePlanGateway for LocalResourceClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.require_provider(&plan.resource.provider_id)?
            .collect_read_plan(plan)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.require_provider(&plan.resource.provider_id)?
            .snapshot_read_plan(plan, kind_id, schema)
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.require_provider(&plan.resource.provider_id)?
            .open_stream_plan(plan)
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.require_provider(&plan.resource.provider_id)?
            .execute_export_plan(plan)
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.require_provider(&plan.resource.provider_id)?
            .commit_write_plan(plan, bytes)
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.require_provider(&plan.capability.provider_id)?
            .execute_command_plan(plan)
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        let provider_id = Self::single_command_provider(batch.commands.iter())?;
        self.require_provider(&provider_id)?
            .execute_command_batch(batch)
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        let provider_id =
            Self::single_command_provider(saga.steps.iter().chain(saga.compensations.iter()))?;
        self.require_provider(&provider_id)?.execute_saga_plan(saga)
    }
}
