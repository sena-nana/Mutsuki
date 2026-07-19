mod batch;
mod common;
mod error;
mod event;
mod execution_policy;
mod extension;
mod observability;
mod plugin;
mod portability;
pub mod resource;
mod runner;
mod task;
mod trace;

pub use batch::{
    BatchEntry, BatchPayload, BinaryPackedPayload, ColumnPayload, ColumnarPayload, CompletionBatch,
    DeferredResourceOp, DispatchLane, EntryCompletion, LocalTaskPayload, OrderingRequirement,
    PayloadLayout, ResourceAccessMode, ResourceBackedPayload, ResourceReadView,
    ResourceRequirement, ResourceSlice, ResourceWriteLock, RowPayload, TaskBatch, WorkBatch,
    WorkResourcePlan, WorkSet,
};
pub use common::{
    BatchId, BatchKey, BindingId, EntryId, ExecutorId, PayloadIndex, PluginId, ProtocolId, RefId,
    ResourceCellId, ResourceLeaseId, RunnerId, ScalarValue, SpanId, SurfaceId, TaskId, TaskLeaseId,
    TickId, TraceId,
};
pub use error::{
    ERR_CAPABILITY_EXHAUSTED, ERR_CHECKPOINT_INCOMPATIBLE, ERR_EXECUTION_NO_VARIANT,
    ERR_PLUGIN_DISABLED, ERR_PLUGIN_NOT_FOUND, ERR_PORTABLE_SCHEMA_UNSUPPORTED,
    ERR_REGISTRY_FROZEN, ERR_REGISTRY_GENERATION_MISMATCH, ERR_REGISTRY_UNAUTHORIZED,
    ERR_RELOAD_BLOCKED, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_LEASE_EXPIRED,
    ERR_RESOURCE_NOT_FOUND, ERR_RESOURCE_UNSUPPORTED, ERR_RUNNER_AWAITABLE_UNSUPPORTED,
    ERR_RUNNER_NOT_FOUND, ERR_RUNNER_PURITY_VIOLATION, ERR_RUNTIME_ABORTED,
    ERR_RUNTIME_HOST_FAILED, ERR_RUNTIME_HOST_GENERATION_MISMATCH, ERR_RUNTIME_NOT_ACCEPTING,
    ERR_STATE_CONFLICT, ERR_TASK_CLAIM_CONFLICT, ERR_TASK_DEAD_LETTER, ERR_TASK_DUPLICATE,
    ERR_TASK_EXPIRED, ERR_TASK_NOT_FOUND, ERR_TASK_UNSUPPORTED, RuntimeError,
};
pub use event::{RuntimeEvent, RuntimeEventKind};
pub use execution_policy::{
    CachePolicy, CapabilitySet, Criticality, DeadlinePolicy, EXECUTION_PROFILE_HISTOGRAM_BUCKETS,
    EXECUTION_PROFILE_WINDOW_CAPACITY, ExecutionOutcomeMetadata, ExecutionPolicy, ExecutionProfile,
    ExecutionProfileAccumulator, ExecutionSample, ExecutionVariant, ExecutionVariantCatalog,
    FailureMode, FixedEwma, FixedHistogram, FixedSampleWindow, LatencyClass, LocalSchedulingHint,
    NoPlacementPolicy, PartialResultPolicy, PressureLevel, ProfilingBudget, QualityPolicy,
    RequirementSet, VariantSelection,
};
pub use extension::{
    BridgeDescriptor, CodecDescriptor, HostExtensionDescriptor, HostExtensionKind,
    PluginBackendDescriptor, SchedulerPolicyDescriptor, WorkflowDescriptor,
};
pub use observability::{
    DEFAULT_EVENT_CAPACITY, DEFAULT_TRACE_CAPACITY, ObservabilityOutletProfile,
    ObservabilityOverflowPolicy, ObservabilityPage, ObservabilityProfile,
};
pub use plugin::{
    ArtifactType, CapabilityProviderSelection, ContractSurface, ContractSurfaceKind,
    HandlerBinding, LifecyclePolicy, PermissionAuditEntry, PermissionGrant, PluginArtifact,
    PluginBusinessSurface, PluginDeploymentKind, PluginManifest, PluginProvides, ProtocolClass,
    ProtocolDescriptor, RuntimeCapabilityGraph, RuntimeLoadPlan, RuntimeLock, RuntimeProfile,
    RuntimeProfileMode, SurfaceCompatibility, SurfaceOccupancy, SurfaceOccupancyHandle,
    SurfaceOccupancyHandleKind,
};
pub use portability::{
    ContentId, ExecutionMobility, PORTABLE_TASK_ENVELOPE_SCHEMA_ID,
    PORTABLE_TASK_ENVELOPE_SCHEMA_VERSION, PortabilityCapability, PortabilityCatalog,
    PortableResourceDescriptor, PortableTask, RecoveryMode, ResourcePersistence, RetrySafety,
    SchemaIdentity, TASK_CHECKPOINT_ENVELOPE_SCHEMA_ID, TASK_CHECKPOINT_ENVELOPE_SCHEMA_VERSION,
    TaskAcceptanceDurability, TaskCheckpoint, TaskPortabilityDescriptor,
};
pub use resource::{
    CommandPlan, ExclusiveWriteLease, ExportPlan, LeaseToken, PatchDescriptor, PlanReceipt,
    ReadPlan, ResourceAccess, ResourceCellRef, ResourceId, ResourceLease, ResourceLifetime,
    ResourceProviderCompatibility, ResourceProviderReloadPolicy, ResourceRef, ResourceSealState,
    ResourceSemantic, ResourceTypeDescriptor, ResourceValue, SnapshotDescriptor, StreamPlan,
    ValueRef, ValueStorage, WritePlan,
};
pub use runner::{
    AsyncInvocation, AsyncInvocationHandle, DomainEvent, EffectPrecondition, EffectRequest,
    ExecutionClass, InvocationMode, RunnerBatchCapability, RunnerConcurrency, RunnerContext,
    RunnerControlCapability, RunnerDescriptor, RunnerMode, RunnerOrderingCapability,
    RunnerPayloadCapability, RunnerPurity, RunnerResourceCapability, RunnerResult,
    RunnerSideEffect, RunnerStatus, TimeoutGranularity,
};
pub use task::{
    CancelPolicy, ConflictPolicy, StateDelta, StateRef, Task, TaskAwait, TaskHandle, TaskLease,
    TaskOutcome, TaskStatus, TaskStepContinuation, VersionExpectation, WakeCondition,
};
pub use trace::{SpanStatus, TraceSpan};

#[cfg(test)]
mod tests;
