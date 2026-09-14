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

/// Awaitable Host registry access. Creation succeeds only after the actor has
/// registered the provider's descriptor. Dropping a future after admission does
/// not cancel committed provider effects or their actor application.
///
/// Blocking providers use the Host executor; Inline remains reserved for bounded
/// in-memory work. Providers never receive a Core handle.
pub trait AsyncResourceRegistryGateway: Send + Sync {
    fn open_resource_descriptor(&self, ref_id: String) -> BoxRuntimeFuture<ResourceRef>;
    fn create_blob_resource(
        &self,
        provider_id: String,
        schema: String,
        bytes: Vec<u8>,
    ) -> BoxRuntimeFuture<ResourceRef>;
    fn create_cow_state_resource(
        &self,
        provider_id: String,
        kind_id: String,
        schema: String,
        bytes: Vec<u8>,
    ) -> BoxRuntimeFuture<ResourceRef>;
    fn create_capability_resource(
        &self,
        provider_id: String,
        kind_id: String,
        schema: String,
    ) -> BoxRuntimeFuture<ResourceRef>;
}

/// Where the Host runs a synchronous provider's plans.
///
/// Core executes resource commands on its actor thread, which is also the only
/// thread that schedules tasks. That is the right place for a provider that
/// only touches memory, and the wrong place for one that blocks on a disk or a
/// socket: the whole runtime stalls for as long as the call takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResourceProviderExecution {
    /// Run on the actor thread. Correct for providers whose calls are
    /// microseconds of in-memory work.
    #[default]
    Inline,
    /// Run on the Host async executor so the actor keeps scheduling. The
    /// calling worker still waits for its reply; only the actor is released.
    /// Requires an async executor; unavailable capacity is a structured failure.
    Offloaded,
}

pub trait ResourceProviderGateway: Send + Sync {
    /// Report committed lifecycle effects even when the operation failed.
    fn execute(
        &self,
        request: ResourceProviderRequest,
    ) -> ResourceProviderOutcome<ResourceProviderReply>;

    /// Invalidating providers must use Ordered; the actor owns the lane through application.
    fn ordering(&self) -> ResourceProviderOrdering {
        ResourceProviderOrdering::Concurrent
    }

    /// Descriptors the provider still holds from an earlier run. The Host
    /// re-registers these once at boot so a persistent provider's resources
    /// stay reachable through `open_resource` after a restart.
    ///
    /// A provider whose storage dies with the process returns nothing, which is
    /// the default.
    ///
    /// # Errors
    ///
    /// Returns a structured failure when the stored descriptors cannot be read.
    fn restore_descriptors(&self) -> RuntimeResult<Vec<ResourceRef>> {
        Ok(Vec::new())
    }

    /// Where the Host should run this provider's plans. A provider that blocks
    /// returns [`ResourceProviderExecution::Offloaded`]; the default keeps
    /// in-memory providers on the actor thread where they are cheapest.
    fn execution(&self) -> ResourceProviderExecution {
        ResourceProviderExecution::Inline
    }
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

pub trait AsyncResourceProviderGateway: Send + Sync {
    fn execute(&self, request: ResourceProviderRequest) -> ResourceProviderFuture;

    /// Invalidating providers must use Ordered; the actor owns the lane through application.
    fn ordering(&self) -> ResourceProviderOrdering {
        ResourceProviderOrdering::Concurrent
    }

    /// See [`ResourceProviderGateway::restore_descriptors`]. Boot-time recovery
    /// runs before the actor starts, so it stays synchronous on both gateways.
    ///
    /// # Errors
    ///
    /// Returns a structured failure when the stored descriptors cannot be read.
    fn restore_descriptors(&self) -> RuntimeResult<Vec<ResourceRef>> {
        Ok(Vec::new())
    }
}

/// Ordered providers retain their lane until the actor applies the result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResourceProviderOrdering {
    #[default]
    Concurrent,
    Ordered,
}

/// Lifecycle effects are independent of success, including partial batch/saga failure.
#[derive(Debug)]
pub struct ResourceProviderOutcome<T> {
    pub result: RuntimeResult<T>,
    pub invalidations: Vec<mutsuki_runtime_contracts::ResourceDescriptorInvalidation>,
}

impl<T> ResourceProviderOutcome<T> {
    pub fn new(result: RuntimeResult<T>) -> Self {
        Self {
            result,
            invalidations: Vec::new(),
        }
    }
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ResourceProviderOutcome<U> {
        ResourceProviderOutcome {
            result: self.result.map(f),
            invalidations: self.invalidations,
        }
    }
}

pub type ResourceProviderFuture =
    Pin<Box<dyn Future<Output = ResourceProviderOutcome<ResourceProviderReply>> + Send + 'static>>;

pub use mutsuki_runtime_contracts::{ResourceProviderReply, ResourceProviderRequest};
