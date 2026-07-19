use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, SnapshotDescriptor, StreamPlan,
    WritePlan,
};
use mutsuki_runtime_core::RuntimeResult;
use std::future::Future;
use std::pin::Pin;

pub type BoxRuntimeFuture<T> = Pin<Box<dyn Future<Output = RuntimeResult<T>> + Send + 'static>>;

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

/// Native async resource plan boundary. Provider-owned futures are driven by
/// the Host async executor and never polled by Core.
pub trait AsyncResourcePlanGateway: Send + Sync {
    fn collect_read_plan(&self, plan: ReadPlan) -> BoxRuntimeFuture<Vec<u8>>;
    fn snapshot_read_plan(
        &self,
        plan: ReadPlan,
        kind_id: String,
        schema: String,
    ) -> BoxRuntimeFuture<SnapshotDescriptor>;
    fn open_stream_plan(&self, plan: ReadPlan) -> BoxRuntimeFuture<StreamPlan>;
    fn execute_export_plan(&self, plan: ExportPlan) -> BoxRuntimeFuture<PlanReceipt>;
    fn commit_write_plan(&self, plan: WritePlan, bytes: Vec<u8>) -> BoxRuntimeFuture<PlanReceipt>;
    fn execute_command_plan(&self, plan: CommandPlan) -> BoxRuntimeFuture<PlanReceipt>;
    fn execute_command_batch(&self, batch: CommandBatch) -> BoxRuntimeFuture<Vec<PlanReceipt>>;
    fn execute_saga_plan(&self, saga: SagaPlan) -> BoxRuntimeFuture<Vec<PlanReceipt>>;
}

pub trait AsyncResourceProviderGateway: AsyncResourcePlanGateway {
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
