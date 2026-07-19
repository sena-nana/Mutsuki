use mutsuki_runtime_contracts::{BatchEntry, CompletionBatch, TaskLease, WorkBatch};

use crate::RuntimeResult;
use crate::runner::{AsyncBatchHandler, Runner};
use mutsuki_runtime_contracts::RunnerContext;
use std::sync::Arc;

pub enum RunnerDispatchTarget {
    Sync(Box<dyn Runner>),
    Async(Arc<dyn AsyncBatchHandler>),
}

impl RunnerDispatchTarget {
    pub fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        match self {
            Self::Sync(runner) => runner.descriptor(),
            Self::Async(handler) => handler.descriptor(),
        }
    }

    pub fn is_async(&self) -> bool {
        matches!(self, Self::Async(_))
    }

    pub fn isolation(&self) -> crate::RunnerIsolation {
        match self {
            Self::Sync(runner) => runner.isolation(),
            Self::Async(handler) => handler.isolation(),
        }
    }

    pub fn management_handle(&self) -> Option<Arc<dyn crate::RunnerManagementHandle>> {
        match self {
            Self::Sync(runner) => runner.management_handle(),
            Self::Async(handler) => handler.management_handle(),
        }
    }
}

pub struct RunnerDispatch {
    pub target: RunnerDispatchTarget,
    pub ctx: RunnerContext,
    pub task_leases: Vec<TaskLease>,
    pub batch: WorkBatch,
}

pub struct RunnerCompletion {
    pub runner: Option<Box<dyn Runner>>,
    pub task_leases: Vec<TaskLease>,
    pub batch_id: String,
    pub expected_entries: Vec<BatchEntry>,
    pub result: RuntimeResult<CompletionBatch>,
}

pub(super) trait RunnerExecutor {
    fn execute(&mut self, dispatch: RunnerDispatch) -> RunnerCompletion;
}

pub(super) struct InlineRunnerExecutor;

impl RunnerExecutor for InlineRunnerExecutor {
    fn execute(&mut self, dispatch: RunnerDispatch) -> RunnerCompletion {
        let RunnerDispatch {
            target,
            ctx,
            task_leases,
            batch,
        } = dispatch;
        let batch_id = batch.batch_id.clone();
        let expected_entries = batch.entries.clone();
        match target {
            RunnerDispatchTarget::Sync(mut runner) => {
                let result = runner.run_batch(ctx, batch);
                RunnerCompletion {
                    runner: Some(runner),
                    task_leases,
                    batch_id,
                    expected_entries,
                    result,
                }
            }
            RunnerDispatchTarget::Async(_handler) => RunnerCompletion {
                runner: None,
                task_leases,
                batch_id,
                expected_entries,
                result: Err(crate::runtime_failure(
                    mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                    "runtime.inline_executor",
                    "host.async_executor.unavailable",
                )),
            },
        }
    }
}
