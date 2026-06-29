use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SagaPlan, SnapshotDescriptor,
    StreamPlan, Task, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};
use mutsuki_runtime_sdk::{ResourceBackend, TaskSubmitter};

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
    runtime: Arc<Mutex<CoreRuntime>>,
}

impl LocalResourceClient {
    pub fn new(runtime: Arc<Mutex<CoreRuntime>>) -> Self {
        Self { runtime }
    }
}

impl ResourceBackend for LocalResourceClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .collect_read_plan(plan)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .snapshot_read_plan(plan, kind_id, schema)
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .open_stream_plan(plan)
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .execute_export_plan(plan)
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .commit_write_plan(plan, bytes)
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .execute_command_plan(plan)
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .execute_command_batch(batch)
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.runtime
            .lock()
            .expect("runtime mutex poisoned")
            .execute_saga_plan(saga)
    }
}
