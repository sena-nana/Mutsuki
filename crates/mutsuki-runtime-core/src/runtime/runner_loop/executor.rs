use mutsuki_runtime_contracts::{CompletionBatch, TaskLease, WorkBatch};

use crate::RuntimeResult;
use crate::runner::Runner;
use mutsuki_runtime_contracts::RunnerContext;

pub struct RunnerDispatch {
    pub runner: Box<dyn Runner>,
    pub ctx: RunnerContext,
    pub task_leases: Vec<TaskLease>,
    pub batch: WorkBatch,
}

pub struct RunnerCompletion {
    pub runner: Box<dyn Runner>,
    pub task_leases: Vec<TaskLease>,
    pub batch_id: String,
    pub result: RuntimeResult<CompletionBatch>,
}

pub(super) trait RunnerExecutor {
    fn execute(&mut self, dispatch: RunnerDispatch) -> RunnerCompletion;
}

pub(super) struct InlineRunnerExecutor;

impl RunnerExecutor for InlineRunnerExecutor {
    fn execute(&mut self, dispatch: RunnerDispatch) -> RunnerCompletion {
        let RunnerDispatch {
            mut runner,
            ctx,
            task_leases,
            batch,
        } = dispatch;
        let batch_id = batch.batch_id.clone();
        let result = runner.run_batch(ctx, batch);
        RunnerCompletion {
            runner,
            task_leases,
            batch_id,
            result,
        }
    }
}
