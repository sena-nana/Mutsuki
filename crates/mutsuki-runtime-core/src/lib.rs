// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::manual_assert_eq,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_pass_by_value,
    clippy::redundant_closure,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::unnested_or_patterns,
    clippy::unused_self,
    clippy::zero_sized_map_values
)]

mod error;
mod id;
mod logs;
mod registry;
mod resource_manager;
mod runner;
mod runtime;
mod state_store;
mod task_pool;
mod trace;

pub use error::{RuntimeFailure, RuntimeResult};
pub(crate) use error::{runtime_error, runtime_failure};
pub use id::{IdSource, SequentialIdSource};
pub use logs::{EventLog, TraceLog};
pub use mutsuki_runtime_contracts::{
    DEFAULT_EVENT_CAPACITY, DEFAULT_TRACE_CAPACITY, RunnerContext,
};
pub use registry::{
    ContractChange, DisposeBag, HandlerBindingRegistry, PluginGenerationPhase,
    PluginGenerationState, RegistrySnapshot, ReloadDecision, RunnerRegistry,
};
pub use resource_manager::{PackedValue, ResourceManager};
pub use runner::{
    AsyncBatchHandler, AsyncCompletionFuture, CoreKernelRunner, Runner, RunnerIsolation,
    RunnerLoopReport, RunnerManagementHandle, RunnerTerminationHandle,
};
pub use runtime::{
    CoreRuntime, DispatchBudget, InvocationPollution, LaneBudget, RunnerCompletion, RunnerDispatch,
    RunnerDispatchTarget, RunningInvocationDisposition, RuntimeStatistics, RuntimeStopState,
    ScheduleDecision, TaskResultSnapshot,
};
pub use task_pool::{RunnerLoad, TaskHistoryRetention, TaskPool, TaskPoolStatistics, TaskRecord};
pub use trace::{TraceClosureIssue, validate_trace_closure};

#[cfg(test)]
mod tests;
