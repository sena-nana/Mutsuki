mod abi;
mod local;

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SagaPlan, SnapshotDescriptor,
    StreamPlan, Task, TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::RuntimeResult;

pub use abi::{AbiResourceClient, AbiTaskClient};
pub use local::{LocalResourceClient, LocalTaskClient};

pub trait TaskClient {
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle>;
    fn cancel_task(&self, task_id: &str) -> RuntimeResult<()>;
    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>>;
}

pub trait ResourcePlanClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>>;
    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor>;
    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan>;
    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt>;
    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt>;
    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt>;
    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>>;
    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>>;
}
