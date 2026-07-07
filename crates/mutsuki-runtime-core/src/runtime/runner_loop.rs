use mutsuki_runtime_contracts::{RunnerDescriptor, RuntimeEventKind, ScalarValue, Task, TaskLease};

use crate::RunnerLoopReport;
use crate::RuntimeResult;
use crate::task_pool::{RunnerLoad, TASK_LEASE_TTL_STEPS};

use super::{CoreRuntime, ScheduleDecision};
use executor::{InlineRunnerExecutor, RunnerExecutor};
pub use executor::{RunnerCompletion, RunnerDispatch};

mod batch;
mod completion_router;
mod dispatch_build;
mod dispatch_selection;
mod executor;
mod failure_reporting;
mod kernel;
mod result_router;
mod trace_metadata;

impl CoreRuntime {
    pub fn tick_once(&mut self) -> RuntimeResult<RunnerLoopReport> {
        let mut executor = InlineRunnerExecutor;
        self.tick_once_with_executor(&mut executor)
    }

    fn tick_once_with_executor(
        &mut self,
        executor: &mut impl RunnerExecutor,
    ) -> RuntimeResult<RunnerLoopReport> {
        self.current_step += 1;
        self.reclaim_expired_task_leases();
        self.wake_due_tasks();
        let mut loop_report = empty_runner_loop_report();
        loop_report.completed_tasks += self.reject_stale_ready_tasks()?;
        let descriptors = self.registry.descriptors();
        for descriptor in descriptors {
            if !dispatch_selection::runner_can_dispatch(&descriptor) {
                continue;
            }
            let load = self.tasks.runner_load(
                &descriptor,
                self.current_step,
                self.load_plan.registry_generation,
            );
            let decision =
                ScheduleDecision::new("core.inline", load.queued_count, "inline.default")
                    .clamp_to(load.queued_count);
            let (report, dispatches) = self.claim_runner_work(
                descriptor,
                decision,
                Some(self.current_step + TASK_LEASE_TTL_STEPS),
            )?;
            loop_report.claimed_tasks += report.claimed_tasks;
            loop_report.completed_tasks += report.completed_tasks;
            for dispatch in dispatches {
                loop_report.completed_tasks +=
                    self.complete_inline_dispatch(executor.execute(dispatch))?;
            }
        }
        Ok(loop_report)
    }

    pub fn claim_ready_dispatches(
        &mut self,
        mut decide_schedule: impl FnMut(
            &RunnerDescriptor,
            &RunnerLoad,
            u64,
            u64,
        ) -> RuntimeResult<ScheduleDecision>,
        lease_expires_at: Option<u64>,
    ) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
        self.current_step += 1;
        self.reclaim_expired_task_leases();
        self.wake_due_tasks();
        let mut loop_report = empty_runner_loop_report();
        loop_report.completed_tasks += self.reject_stale_ready_tasks()?;
        let mut dispatches = Vec::new();
        let descriptors = self.registry.descriptors();
        for descriptor in descriptors {
            if !dispatch_selection::runner_can_dispatch(&descriptor) {
                continue;
            }
            let load = self.tasks.runner_load(
                &descriptor,
                self.current_step,
                self.load_plan.registry_generation,
            );
            let decision = decide_schedule(
                &descriptor,
                &load,
                self.current_step,
                self.load_plan.registry_generation,
            )?;
            let (report, runner_dispatches) =
                self.claim_runner_work(descriptor, decision, lease_expires_at)?;
            loop_report.claimed_tasks += report.claimed_tasks;
            loop_report.completed_tasks += report.completed_tasks;
            dispatches.extend(runner_dispatches);
        }
        Ok((loop_report, dispatches))
    }

    fn claim_runner_work(
        &mut self,
        descriptor: RunnerDescriptor,
        decision: ScheduleDecision,
        lease_expires_at: Option<u64>,
    ) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
        dispatch_selection::claim_runner_work(self, descriptor, decision, lease_expires_at)
    }

    fn build_runner_dispatch(
        &mut self,
        descriptor: &RunnerDescriptor,
        executor_id: String,
        leased_tasks: Vec<(TaskLease, Task)>,
    ) -> RuntimeResult<RunnerDispatch> {
        dispatch_build::build_runner_dispatch(self, descriptor, executor_id, leased_tasks)
    }

    fn complete_inline_dispatch(&mut self, completion: RunnerCompletion) -> RuntimeResult<usize> {
        Ok(self.complete_runner_dispatch(completion)?.completed_tasks)
    }

    fn record_scheduler_decision(
        &mut self,
        descriptor: &RunnerDescriptor,
        decision: &ScheduleDecision,
    ) {
        let mut attrs = std::collections::BTreeMap::from([
            (
                "scheduler_id".into(),
                ScalarValue::String(decision.scheduler_id.clone()),
            ),
            (
                "runner_id".into(),
                ScalarValue::String(descriptor.runner_id.clone()),
            ),
            (
                "requested_dispatch_limit".into(),
                ScalarValue::Int(decision.requested_dispatch_limit as i64),
            ),
            (
                "effective_dispatch_limit".into(),
                ScalarValue::Int(decision.dispatch_limit as i64),
            ),
            (
                "budget_max_entries".into(),
                ScalarValue::Int(decision.budget.max_entries as i64),
            ),
            (
                "budget_max_batches".into(),
                ScalarValue::Int(decision.budget.max_batches as i64),
            ),
            (
                "reason".into(),
                ScalarValue::String(decision.reason.clone()),
            ),
            (
                "registry_generation".into(),
                ScalarValue::Int(self.load_plan.registry_generation as i64),
            ),
            (
                "current_step".into(),
                ScalarValue::Int(self.current_step as i64),
            ),
        ]);
        let span = self.traces.record(
            format!("trace-scheduler-{}", descriptor.runner_id),
            "scheduler.decision",
            None,
            mutsuki_runtime_contracts::SpanStatus::Ok,
            attrs.clone(),
        );
        attrs.insert("span_id".into(), ScalarValue::String(span.span_id));
        self.events.record(
            RuntimeEventKind::Trace,
            "scheduler.decision",
            Some(descriptor.runner_id.clone()),
            attrs,
            None,
        );
    }

    pub fn complete_runner_dispatch(
        &mut self,
        completion: RunnerCompletion,
    ) -> RuntimeResult<RunnerLoopReport> {
        completion_router::complete_runner_dispatch(self, completion)
    }

    fn reclaim_expired_task_leases(&mut self) {
        failure_reporting::reclaim_expired_task_leases(self);
    }

    pub fn run_until_idle(&mut self, max_ticks: usize) -> RuntimeResult<RunnerLoopReport> {
        let mut aggregate = RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: 0,
        };
        for _ in 0..max_ticks {
            let report = self.tick_once()?;
            aggregate.claimed_tasks += report.claimed_tasks;
            aggregate.completed_tasks += report.completed_tasks;
            if report.claimed_tasks == 0 && report.completed_tasks == 0 {
                break;
            }
        }
        Ok(aggregate)
    }
}

fn empty_runner_loop_report() -> RunnerLoopReport {
    RunnerLoopReport {
        claimed_tasks: 0,
        completed_tasks: 0,
    }
}
