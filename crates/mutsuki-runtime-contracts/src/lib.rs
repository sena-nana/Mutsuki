mod batch;
mod common;
mod error;
mod event;
mod extension;
mod plugin;
mod resource;
mod runner;
mod task;
mod trace;

pub use batch::{
    BatchEntry, BatchPayload, ColumnPayload, ColumnarPayload, CompletionBatch, DeferredResourceOp,
    DispatchLane, EntryCompletion, OrderingRequirement, PackedBuffer, PayloadLayout,
    ResourceAccessMode, ResourceReadView, ResourceRequirement, ResourceSlice, ResourceSliceSet,
    ResourceWriteLock, TaskBatch, WorkBatch, WorkResourcePlan, WorkSet,
};
pub use common::{
    BatchId, BatchKey, BindingId, EntryId, ExecutorId, PayloadIndex, PluginId, ProtocolId, RefId,
    ResourceCellId, ResourceLeaseId, RunnerId, ScalarValue, SpanId, SurfaceId, TaskId, TaskLeaseId,
    TickId, TraceId,
};
pub use error::{
    ERR_CAPABILITY_EXHAUSTED, ERR_PLUGIN_DISABLED, ERR_PLUGIN_NOT_FOUND, ERR_REGISTRY_FROZEN,
    ERR_REGISTRY_GENERATION_MISMATCH, ERR_REGISTRY_UNAUTHORIZED, ERR_RELOAD_BLOCKED,
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_LEASE_EXPIRED, ERR_RESOURCE_NOT_FOUND,
    ERR_RESOURCE_UNSUPPORTED, ERR_RUNNER_NOT_FOUND, ERR_RUNNER_PURITY_VIOLATION,
    ERR_RUNTIME_HOST_FAILED, ERR_RUNTIME_HOST_GENERATION_MISMATCH, ERR_STATE_CONFLICT,
    ERR_TASK_CLAIM_CONFLICT, ERR_TASK_DEAD_LETTER, ERR_TASK_DUPLICATE, ERR_TASK_EXPIRED,
    ERR_TASK_NOT_FOUND, ERR_TASK_UNSUPPORTED, RuntimeError,
};
pub use event::{RuntimeEvent, RuntimeEventKind};
pub use extension::{
    BridgeDescriptor, CodecDescriptor, HostExtensionDescriptor, HostExtensionKind,
    PluginBackendDescriptor, SchedulerPolicyDescriptor, WorkflowDescriptor,
};
pub use plugin::{
    ArtifactType, CapabilityProviderSelection, ContractSurface, ContractSurfaceKind,
    HandlerBinding, LifecyclePolicy, PermissionAuditEntry, PermissionGrant, PluginArtifact,
    PluginDeploymentKind, PluginManifest, PluginProvides, ProtocolDescriptor,
    RuntimeCapabilityGraph, RuntimeLoadPlan, RuntimeLock, RuntimeProfile, RuntimeProfileMode,
    SurfaceCompatibility, SurfaceOccupancy, SurfaceOccupancyHandle, SurfaceOccupancyHandleKind,
};
pub use resource::{
    CommandBatch, CommandPlan, ExclusiveWriteLease, ExportPlan, LeaseToken, PatchDescriptor,
    PlanReceipt, ReadPlan, ResourceAccess, ResourceCellRef, ResourceId, ResourceLease,
    ResourceLifetime, ResourceProviderCompatibility, ResourceProviderReloadPolicy, ResourceRef,
    ResourceSealState, ResourceSemantic, ResourceTypeDescriptor, ResourceValue, SagaPlan,
    SnapshotDescriptor, StreamPlan, TransactionPlan, ValueRef, ValueStorage, WritePlan,
};
pub use runner::{
    DomainEvent, EffectPrecondition, EffectRequest, ExecutionClass, RunnerBatchCapability,
    RunnerContext, RunnerControlCapability, RunnerDescriptor, RunnerMode, RunnerOrderingCapability,
    RunnerPayloadCapability, RunnerPurity, RunnerResourceCapability, RunnerResult, RunnerStatus,
    TimeoutGranularity,
};
pub use task::{
    CancelPolicy, ConflictPolicy, StateDelta, StateRef, Task, TaskAwait, TaskHandle, TaskLease,
    TaskOutcome, TaskStatus, TaskStepContinuation, VersionExpectation, WakeCondition,
};
pub use trace::{SpanStatus, TraceSpan};

#[cfg(test)]
mod tests;
