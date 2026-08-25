// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::float_cmp,
    clippy::ignore_without_reason,
    clippy::ignored_unit_patterns,
    clippy::inline_always,
    clippy::manual_assert,
    clippy::manual_assert_eq,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::match_wild_err_arm,
    clippy::missing_errors_doc,
    clippy::missing_fields_in_debug,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::semicolon_if_nothing_returned,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_wraps,
    clippy::unnested_or_patterns,
    clippy::useless_conversion,
    clippy::wildcard_imports
)]

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
mod scope;
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
    AttachedEffectHandle, HostAsyncResourceProviders, HostResourceProviders, HostRuntime,
    HostRuntimeConfig, HostRuntimeDriveState, HostRuntimeMetrics, HostRuntimeMetricsSnapshot,
    TaskChangeSubscription, TaskCompletionSubscription,
};
pub use manifest::{runner_manifest, runner_manifest_with_artifact};
pub use mutsuki_runtime_sdk::{HostTaskFailureSummary, HostTaskSnapshot};
pub use native_runner::{CancellationProbe, NativeRunner};
pub use process::{ProcessRunnerSpec, SpawnedBinaryRunner};
pub use resolver::resolve_load_plan;
pub use scheduler::{DefaultScheduler, HostCapacity, RunnerLimits, ScheduleInput, SchedulerPolicy};
pub use scope::{
    AsyncEffect, DisposeReport, EffectFuture, EffectId, EffectKind, EffectSnapshot, PluginLifetime,
    PluginScopeManager, ResolvedService, ScopeEvent, ScopeEventKind, ScopeId, ScopeSnapshot,
    ScopeState, ScopedEffect, ServiceBinding, ServiceChange, ServiceDependency, ServiceKey,
    ServiceRequirement,
};
pub use worker::{
    ExecutionDomainConfig, LaneExecutionPolicy, LaneWorkerSnapshot, WorkerPoolSnapshot,
};

#[cfg(test)]
mod tests;
