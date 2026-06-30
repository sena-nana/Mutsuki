use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, SagaPlan,
    SnapshotDescriptor, StreamPlan, WritePlan,
};
use mutsuki_runtime_core::RuntimeResult;

pub trait ResourcePlanGateway: Send + Sync {
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

pub trait ResourceProviderGateway: ResourcePlanGateway {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef>;
    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef>;
    fn create_capability_resource(&self, kind_id: &str, schema: &str)
    -> RuntimeResult<ResourceRef>;
}
