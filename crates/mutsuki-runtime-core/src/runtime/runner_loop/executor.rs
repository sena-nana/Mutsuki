use mutsuki_runtime_contracts::{RunnerResult, Task, TaskLease};

use crate::RuntimeResult;
use crate::runner::{Runner, RunnerContext};

pub struct RunnerDispatch {
    pub runner: Box<dyn Runner>,
    pub ctx: RunnerContext,
    pub task_leases: Vec<TaskLease>,
    pub tasks: Vec<Task>,
}

pub struct RunnerCompletion {
    pub runner: Box<dyn Runner>,
    pub task_leases: Vec<TaskLease>,
    pub results: RuntimeResult<Vec<RunnerResult>>,
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
            tasks,
        } = dispatch;
        let results = runner.step(ctx, tasks);
        RunnerCompletion {
            runner,
            task_leases,
            results,
        }
    }
}
