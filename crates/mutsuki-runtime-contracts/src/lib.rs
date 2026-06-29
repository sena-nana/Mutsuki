mod common;
mod error;
mod event;
mod extension;
mod plugin;
mod resource;
mod runner;
mod task;
mod trace;

pub use common::{
    BindingId, ExecutorId, PluginId, ProtocolId, RefId, ResourceCellId, ResourceLeaseId, RunnerId,
    ScalarValue, SpanId, SurfaceId, TaskId, TaskLeaseId, TraceId,
};
pub use error::{
    ERR_CAPABILITY_EXHAUSTED, ERR_PLUGIN_DISABLED, ERR_PLUGIN_NOT_FOUND, ERR_REGISTRY_FROZEN,
    ERR_REGISTRY_GENERATION_MISMATCH, ERR_REGISTRY_UNAUTHORIZED, ERR_RELOAD_BLOCKED,
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_LEASE_EXPIRED, ERR_RESOURCE_NOT_FOUND,
    ERR_RUNNER_NOT_FOUND, ERR_RUNNER_PURITY_VIOLATION, ERR_RUNTIME_HOST_FAILED,
    ERR_RUNTIME_HOST_GENERATION_MISMATCH, ERR_STATE_CONFLICT, ERR_TASK_CLAIM_CONFLICT,
    ERR_TASK_NOT_FOUND, RuntimeError,
};
pub use event::{RuntimeEvent, RuntimeEventKind};
pub use extension::{
    BridgeDescriptor, CodecDescriptor, HostBackendDescriptor, HostExtensionKind,
    PluginBackendDescriptor, SchedulerPolicyDescriptor, WorkflowDescriptor,
};
pub use plugin::{
    ArtifactType, ContractSurface, ContractSurfaceKind, HandlerBinding, LifecyclePolicy,
    PermissionGrant, PluginArtifact, PluginDeploymentKind, PluginManifest, PluginProvides,
    ProtocolDescriptor, RuntimeCapabilityGraph, RuntimeLoadPlan, RuntimeLock, RuntimeProfile,
    RuntimeProfileMode, SurfaceCompatibility, SurfaceOccupancy, SurfaceOccupancyHandle,
    SurfaceOccupancyHandleKind,
};
pub use resource::{
    CommandBatch, CommandPlan, ExclusiveWriteLease, ExportPlan, LeaseToken, PatchDescriptor,
    PlanReceipt, ReadPlan, ResourceAccess, ResourceCellRef, ResourceId, ResourceLease,
    ResourceLifetime, ResourceProviderCompatibility, ResourceProviderReloadPolicy, ResourceRef,
    ResourceSealState, ResourceSemantic, ResourceTypeDescriptor, ResourceValue, SagaPlan,
    SnapshotDescriptor, StreamPlan, TransactionPlan, ValueRef, ValueStorage, WritePlan,
};
pub use runner::{
    DomainEvent, EffectPrecondition, EffectRequest, ExecutionClass, RunnerContext,
    RunnerDescriptor, RunnerPurity, RunnerResult, RunnerStatus,
};
pub use task::{
    CancelPolicy, ConflictPolicy, StateDelta, StateRef, Task, TaskAwait, TaskHandle, TaskLease,
    TaskOutcome, TaskStatus, TaskStepContinuation, VersionExpectation, WakeCondition,
};
pub use trace::{SpanStatus, TraceSpan};

#[cfg(test)]
mod tests;
