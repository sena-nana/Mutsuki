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
mod resolver;
mod runtime_context;
mod scheduler;
mod worker;

pub use backend::{HostExtension, PluginBackend};
pub use bootstrapper::{NativeRunner, RuntimeBootstrapper};
pub use capabilities::HostCapabilityRegistry;
pub use clients::{
    AbiResourceClient, AbiTaskClient, LocalResourceClient, LocalTaskClient, ResourcePlanClient,
    ResourcePlanProvider, TaskClient,
};
pub use commands::{HostRuntimeCommand, HostRuntimeReply};
pub use host::{HostRuntime, HostRuntimeConfig};
pub use jsonl::JsonlRunner;
pub use manifest::{runner_manifest, runner_manifest_with_artifact};
pub use resolver::resolve_load_plan;
pub use scheduler::{DefaultScheduler, RunnerLimits, ScheduleInput, SchedulerPolicy};

#[cfg(test)]
mod tests;
