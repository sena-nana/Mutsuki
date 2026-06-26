mod common;
mod error;
mod event;
mod plugin;
mod resource;
mod runner;
mod task;
mod trace;

pub use common::{PluginId, RefId, RunnerId, ScalarValue, SpanId, SurfaceId, TaskId, TraceId};
pub use error::{
    ERR_CAPABILITY_EXHAUSTED, ERR_PLUGIN_DISABLED, ERR_PLUGIN_NOT_FOUND, ERR_REGISTRY_FROZEN,
    ERR_REGISTRY_GENERATION_MISMATCH, ERR_REGISTRY_UNAUTHORIZED, ERR_RELOAD_BLOCKED,
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_LEASE_EXPIRED, ERR_RESOURCE_NOT_FOUND,
    ERR_RUNNER_NOT_FOUND, ERR_RUNNER_PURITY_VIOLATION, ERR_RUNTIME_HOST_FAILED,
    ERR_RUNTIME_HOST_GENERATION_MISMATCH, ERR_STATE_CONFLICT, ERR_TASK_CLAIM_CONFLICT,
    ERR_TASK_NOT_FOUND, RuntimeError,
};
pub use event::{RuntimeEvent, RuntimeEventKind};
pub use plugin::{
    ArtifactType, ContractSurface, ContractSurfaceKind, HandlerBinding, LifecyclePolicy,
    PermissionGrant, PluginArtifact, PluginManifest, PluginProvides, ProtocolDescriptor,
    RuntimeLoadPlan, RuntimeLock, RuntimeProfile, SurfaceCompatibility, SurfaceOccupancy,
    SurfaceOccupancyHandle, SurfaceOccupancyHandleKind,
};
pub use resource::{
    ExclusiveWriteLease, LeaseToken, ResourceAccess, ResourceLifetime, ResourceRef,
    ResourceSealState, ResourceValue, ValueRef, ValueStorage,
};
pub use runner::{
    DomainEvent, EffectPrecondition, EffectRequest, RunnerDescriptor, RunnerPurity, RunnerResult,
    RunnerStatus,
};
pub use task::{ConflictPolicy, StateDelta, StateRef, Task, TaskStatus, VersionExpectation};
pub use trace::{SpanStatus, TraceSpan};

#[cfg(test)]
mod tests;
