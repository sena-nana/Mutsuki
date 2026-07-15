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
pub use runner::{CoreKernelRunner, Runner, RunnerLoopReport};
pub use runtime::{
    CoreRuntime, DispatchBudget, InvocationPollution, LaneBudget, RunnerCompletion, RunnerDispatch,
    RunningInvocationDisposition, RuntimeStatistics, RuntimeStopState, ScheduleDecision,
    TaskResultSnapshot,
};
pub use task_pool::{RunnerLoad, TaskHistoryRetention, TaskPool, TaskPoolStatistics, TaskRecord};
pub use trace::{TraceClosureIssue, validate_trace_closure};

#[cfg(test)]
mod tests;
