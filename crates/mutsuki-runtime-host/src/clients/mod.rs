mod abi;
mod local;

use mutsuki_runtime_sdk::{ResourceBackend, TaskSubmitter};

pub use abi::{AbiResourceClient, AbiTaskClient};
pub use local::{LocalResourceClient, LocalTaskClient};

pub trait TaskClient: TaskSubmitter {
    fn submit_task(
        &self,
        task: mutsuki_runtime_contracts::Task,
    ) -> mutsuki_runtime_core::RuntimeResult<mutsuki_runtime_contracts::TaskHandle> {
        TaskSubmitter::submit_task(self, task)
    }

    fn cancel_task(&self, task_id: &str) -> mutsuki_runtime_core::RuntimeResult<()> {
        TaskSubmitter::cancel_task(self, task_id)
    }

    fn task_outcome(
        &self,
        task_id: &str,
    ) -> mutsuki_runtime_core::RuntimeResult<Option<mutsuki_runtime_contracts::TaskOutcome>> {
        TaskSubmitter::task_outcome(self, task_id)
    }
}

impl<T> TaskClient for T where T: TaskSubmitter + ?Sized {}

pub trait ResourcePlanClient: ResourceBackend {
    fn collect_read_plan(
        &self,
        plan: &mutsuki_runtime_contracts::ReadPlan,
    ) -> mutsuki_runtime_core::RuntimeResult<Vec<u8>> {
        ResourceBackend::collect_read_plan(self, plan)
    }

    fn snapshot_read_plan(
        &self,
        plan: &mutsuki_runtime_contracts::ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> mutsuki_runtime_core::RuntimeResult<mutsuki_runtime_contracts::SnapshotDescriptor> {
        ResourceBackend::snapshot_read_plan(self, plan, kind_id, schema)
    }

    fn open_stream_plan(
        &self,
        plan: &mutsuki_runtime_contracts::ReadPlan,
    ) -> mutsuki_runtime_core::RuntimeResult<mutsuki_runtime_contracts::StreamPlan> {
        ResourceBackend::open_stream_plan(self, plan)
    }

    fn execute_export_plan(
        &self,
        plan: &mutsuki_runtime_contracts::ExportPlan,
    ) -> mutsuki_runtime_core::RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        ResourceBackend::execute_export_plan(self, plan)
    }

    fn commit_write_plan(
        &self,
        plan: &mutsuki_runtime_contracts::WritePlan,
        bytes: Vec<u8>,
    ) -> mutsuki_runtime_core::RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        ResourceBackend::commit_write_plan(self, plan, bytes)
    }

    fn execute_command_plan(
        &self,
        plan: &mutsuki_runtime_contracts::CommandPlan,
    ) -> mutsuki_runtime_core::RuntimeResult<mutsuki_runtime_contracts::PlanReceipt> {
        ResourceBackend::execute_command_plan(self, plan)
    }

    fn execute_command_batch(
        &self,
        batch: &mutsuki_runtime_contracts::CommandBatch,
    ) -> mutsuki_runtime_core::RuntimeResult<Vec<mutsuki_runtime_contracts::PlanReceipt>> {
        ResourceBackend::execute_command_batch(self, batch)
    }

    fn execute_saga_plan(
        &self,
        saga: &mutsuki_runtime_contracts::SagaPlan,
    ) -> mutsuki_runtime_core::RuntimeResult<Vec<mutsuki_runtime_contracts::PlanReceipt>> {
        ResourceBackend::execute_saga_plan(self, saga)
    }
}

impl<T> ResourcePlanClient for T where T: ResourceBackend + ?Sized {}
