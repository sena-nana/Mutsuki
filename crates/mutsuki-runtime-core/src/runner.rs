use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, RunnerContext, RunnerDescriptor,
    RunnerPurity, RunnerResult, WorkBatch,
};

use crate::RuntimeResult;

pub trait RunnerTerminationHandle: Send + Sync + fmt::Debug {
    /// Terminates the deployment boundary that is currently executing a runner invocation.
    ///
    /// Implementations must only return success after a real termination signal has been
    /// delivered to a boundary that can be killed independently from the Host process.
    fn terminate(&self) -> RuntimeResult<()>;
}

pub trait RunnerManagementHandle: Send + Sync + fmt::Debug {
    fn cancel(&self, invocation_id: &str) -> RuntimeResult<()>;

    fn dispose(&self) -> RuntimeResult<()>;
}

#[derive(Clone)]
pub enum RunnerIsolation {
    /// In-process native code can only observe cooperative cancellation. Rust threads cannot be
    /// forcefully terminated without compromising the Host process.
    Cooperative,
    /// A process or sidecar boundary can be terminated independently and recreated afterwards.
    HardProcess(Arc<dyn RunnerTerminationHandle>),
}

impl fmt::Debug for RunnerIsolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cooperative => formatter.write_str("Cooperative"),
            Self::HardProcess(_) => formatter.write_str("HardProcess"),
        }
    }
}

pub trait Runner: Send {
    fn descriptor(&self) -> &RunnerDescriptor;

    fn run_batch(&mut self, ctx: RunnerContext, batch: WorkBatch)
    -> RuntimeResult<CompletionBatch>;

    fn cancel(&mut self, _invocation_id: &str) -> RuntimeResult<()> {
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        Ok(())
    }

    fn isolation(&self) -> RunnerIsolation {
        RunnerIsolation::Cooperative
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        None
    }

    /// Recreates a runner after its independently terminable deployment boundary was killed.
    /// Native runners intentionally do not implement this method.
    fn recover_after_hard_termination(&mut self) -> RuntimeResult<()> {
        Err(crate::runtime_failure(
            mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
            "runtime.runner",
            format!(
                "runner.{}.hard_recovery_unsupported",
                self.descriptor().runner_id
            ),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerLoopReport {
    pub claimed_tasks: usize,
    pub completed_tasks: usize,
}

pub struct CoreKernelRunner {
    descriptor: RunnerDescriptor,
}

impl CoreKernelRunner {
    pub fn new(plugin_generation: u64) -> Self {
        Self {
            descriptor: RunnerDescriptor {
                runner_id: "core.kernel".into(),
                plugin_id: "core".into(),
                plugin_generation,
                accepted_protocol_ids: vec!["core.commit".into(), "core.event.append".into()],
                purity: RunnerPurity::Committer,
                execution_class: ExecutionClass::Control,
                input_schema: serde_json::json!({}),
                output_schema: serde_json::json!({}),
                batch: Default::default(),
                payload: Default::default(),
                resources: Default::default(),
                ordering: Default::default(),
                control: Default::default(),
                metadata: BTreeMap::new(),
                contract_surfaces: vec!["runner:core.kernel".into()],
            },
        }
    }
}

impl Runner for CoreKernelRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    /// Marker runner used to authorize core.* tasks. Actual kernel task
    /// handling is performed by CoreRuntime before runner dispatch.
    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let results = batch
            .entries
            .iter()
            .map(|entry| EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: Some(RunnerResult::completed(entry.task_id.clone())),
                error: None,
            })
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }
}
