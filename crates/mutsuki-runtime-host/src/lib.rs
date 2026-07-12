mod abi_transport;
mod actor;
mod backend;
mod bootstrapper;
mod capabilities;
mod clients;
mod commands;
mod error;
mod host;
mod jsonl;
mod manifest;
mod process;
mod resolver;
mod resource_router;
mod runtime_context;
mod scheduler;
mod worker;

pub use abi_transport::{JsonRequestTransport, TransportJsonlRunner, TransportResourceProvider};
pub use backend::{HostExtension, PluginBackend};
pub use bootstrapper::{NativeRunner, PreparedRuntimeReload, RuntimeBootstrapper};
pub use capabilities::HostCapabilityRegistry;
pub use clients::{
    AbiResourceClient, AbiTaskClient, LocalResourceClient, LocalTaskClient, ResourcePlanClient,
    ResourcePlanProvider, TaskClient,
};
pub use commands::{HostRuntimeCommand, HostRuntimeReply};
pub use host::{HostRuntime, HostRuntimeConfig};
pub use jsonl::JsonlRunner;
pub use manifest::{runner_manifest, runner_manifest_with_artifact};
pub use mutsuki_runtime_sdk::{HostTaskFailureSummary, HostTaskSnapshot};
pub use process::{ProcessRunnerSpec, SpawnedJsonlRunner};
pub use resolver::resolve_load_plan;
pub use scheduler::{DefaultScheduler, HostCapacity, RunnerLimits, ScheduleInput, SchedulerPolicy};

#[cfg(test)]
mod tests;
