mod abi_plugin;
mod abi_transport;
mod actor;
mod async_executor;
mod backend;
mod binary;
mod bootstrapper;
mod capabilities;
mod clients;
mod commands;
mod error;
mod group;
mod host;
mod management;
mod manifest;
mod multiplexer;
mod native_runner;
mod process;
mod resolver;
mod resource_router;
mod runtime_context;
mod scheduler;
mod worker;

pub use abi_plugin::{AbiPluginLoadRequest, load_abi_plugin_v2};
pub use abi_transport::{TransportResourceProvider, TransportRunner, TypedRequestTransport};
pub use async_executor::{
    AsyncEventSink, AsyncExecutor, AsyncExecutorEvent, AsyncExecutorSnapshot, TokioAsyncExecutor,
};
pub use backend::{HostExtension, PluginBackend};
pub use binary::{BinaryRunner, BinaryTransport};
pub use bootstrapper::{PreparedHostRuntime, PreparedRuntimeReload, RuntimeBootstrapper};
pub use capabilities::HostCapabilityRegistry;
pub use clients::{
    AbiResourceClient, AbiTaskClient, LocalResourceClient, LocalTaskClient, ResourcePlanClient,
    ResourcePlanProvider, TaskClient,
};
pub use commands::{HostRuntimeCommand, HostRuntimeReply, HostTaskState};
pub use group::{
    ExplicitRuntimeDomainRouter, RuntimeDomainRouter, RuntimeDomainSnapshot, RuntimeGroupHost,
    RuntimeGroupLimits,
};
pub use host::{
    HostAsyncResourceProviders, HostResourceProviders, HostRuntime, HostRuntimeConfig,
    HostRuntimeDriveState, HostRuntimeMetrics, HostRuntimeMetricsSnapshot,
    TaskCompletionSubscription,
};
pub use manifest::{runner_manifest, runner_manifest_with_artifact};
pub use mutsuki_runtime_sdk::{HostTaskFailureSummary, HostTaskSnapshot};
pub use native_runner::{CancellationProbe, NativeRunner};
pub use process::{ProcessRunnerSpec, SpawnedBinaryRunner};
pub use resolver::resolve_load_plan;
pub use scheduler::{DefaultScheduler, HostCapacity, RunnerLimits, ScheduleInput, SchedulerPolicy};
pub use worker::{
    ExecutionDomainConfig, LaneExecutionPolicy, LaneWorkerSnapshot, WorkerPoolSnapshot,
};

#[cfg(test)]
mod tests;
