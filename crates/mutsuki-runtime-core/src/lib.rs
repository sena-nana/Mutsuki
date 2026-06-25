mod error;
mod id;
mod logs;
mod registry;
mod resource_manager;
mod runtime;
mod task_pool;
mod trace;

pub use error::{RuntimeFailure, RuntimeResult};
pub use id::{IdSource, SequentialIdSource};
pub use logs::{EventLog, TraceLog};
pub use registry::{
    ContractChange, DisposeBag, PluginGenerationState, RegistrySnapshot, ReloadDecision,
    RunnerRegistry,
};
pub use resource_manager::{PackedValue, ResourceManager};
pub use runtime::{
    CoreKernelRunner, CoreRuntime, DefaultOrchestratorRunner, Runner, RunnerContext,
    RunnerLoopReport,
};
pub use task_pool::{TaskPool, TaskRecord};
pub use trace::{TraceClosureIssue, validate_trace_closure};

#[cfg(test)]
mod tests;
