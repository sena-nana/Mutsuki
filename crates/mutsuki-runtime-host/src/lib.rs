mod abi_transport;
mod actor;
mod backend;
mod binary;
mod bootstrapper;
mod capabilities;
mod clients;
mod commands;
mod error;
mod host;
mod jsonl;
mod management;
mod manifest;
mod multiplexer;
mod process;
mod resolver;
mod resource_router;
mod runtime_context;
mod scheduler;
mod worker;

pub use abi_transport::{
    TransportJsonlRunner, TransportResourceProvider, TransportRunner, TypedRequestTransport,
};
pub use backend::{HostExtension, PluginBackend};
pub use binary::{BinaryRunner, BinaryTransport};
pub use bootstrapper::{NativeRunner, PreparedRuntimeReload, RuntimeBootstrapper};
pub use capabilities::HostCapabilityRegistry;
pub use clients::{
    AbiResourceClient, AbiTaskClient, LocalResourceClient, LocalTaskClient, ResourcePlanClient,
    ResourcePlanProvider, TaskClient,
};
pub use commands::{HostRuntimeCommand, HostRuntimeReply, HostTaskState};
pub use host::{
    HostRuntime, HostRuntimeConfig, HostRuntimeDriveState, HostRuntimeMetricsSnapshot,
    TaskCompletionSubscription,
};
pub use jsonl::{JsonlRunner, JsonlTransport};
pub use manifest::{runner_manifest, runner_manifest_with_artifact};
pub use mutsuki_runtime_sdk::{HostTaskFailureSummary, HostTaskSnapshot};
pub use process::{ProcessRunnerSpec, SpawnedJsonlRunner};
pub use resolver::resolve_load_plan;
pub use scheduler::{DefaultScheduler, HostCapacity, RunnerLimits, ScheduleInput, SchedulerPolicy};
pub use worker::WorkerPoolSnapshot;

#[cfg(test)]
mod tests;
