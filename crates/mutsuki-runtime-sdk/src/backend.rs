use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, SnapshotDescriptor, StreamPlan,
    WritePlan,
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

/// Host-owned aggregate gateway for opening registered resources and creating
/// new resources through an explicitly selected provider.
///
/// The gateway only crosses the SDK boundary with descriptors and bytes. The
/// provider instance and any provider-native handle remain owned by the host.
pub trait ResourceRegistryGateway: ResourcePlanGateway {
    fn open_resource_descriptor(&self, ref_id: &str) -> RuntimeResult<ResourceRef>;
    fn create_blob_resource(
        &self,
        provider_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef>;
    fn create_cow_state_resource(
        &self,
        provider_id: &str,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef>;
    fn create_capability_resource(
        &self,
        provider_id: &str,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef>;
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
