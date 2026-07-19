use mutsuki_runtime_contracts::{
    RunnerDescriptor, RuntimeEventKind, ScalarValue, SpanStatus, Task, TaskLease,
};

use crate::RunnerLoopReport;
use crate::RuntimeResult;
use crate::task_pool::{RunnerLoad, TASK_LEASE_TTL_STEPS};

use super::{CoreRuntime, ScheduleDecision};
use executor::{InlineRunnerExecutor, RunnerExecutor};
pub use executor::{RunnerCompletion, RunnerDispatch, RunnerDispatchTarget};

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
        self.ensure_not_aborted()?;
        self.current_step += 1;
        self.reclaim_expired_task_leases();
        self.wake_due_tasks();
        let mut loop_report = empty_runner_loop_report();
        loop_report.completed_tasks += self.reject_stale_ready_tasks()?;
        let descriptors = self.registry.descriptor_snapshot();
        for descriptor in descriptors.iter() {
            if !dispatch_selection::runner_can_dispatch(descriptor) {
                continue;
            }
            let load = self.tasks.runner_load(
                descriptor,
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
        decide_schedule: impl FnMut(
            &RunnerDescriptor,
            &RunnerLoad,
            u64,
            u64,
        ) -> RuntimeResult<ScheduleDecision>,
        lease_expires_at: Option<u64>,
    ) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
        let next_step = self.current_step.saturating_add(1);
        self.claim_ready_dispatches_at_step(next_step, decide_schedule, lease_expires_at)
    }

    /// Advances directly to a future logical step and performs one scheduling pass.
    ///
    /// This preserves the same due-task, lease and scheduler checks as `claim_ready_dispatches`
    /// while allowing a Host to sleep until the nearest indexed deadline instead of issuing an
    /// empty tick for every skipped step.
    pub fn claim_ready_dispatches_at_step(
        &mut self,
        target_step: u64,
        mut decide_schedule: impl FnMut(
            &RunnerDescriptor,
            &RunnerLoad,
            u64,
            u64,
        ) -> RuntimeResult<ScheduleDecision>,
        lease_expires_at: Option<u64>,
    ) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
        self.ensure_not_aborted()?;
        if target_step <= self.current_step {
            return Err(crate::runtime_failure(
                "runtime.tick.invalid",
                "runtime.runner_loop",
                format!("runtime.step.{target_step}.after.{}", self.current_step),
            ));
        }
        self.current_step = target_step;
        self.reclaim_expired_task_leases();
        self.wake_due_tasks();
        let mut loop_report = empty_runner_loop_report();
        loop_report.completed_tasks += self.reject_stale_ready_tasks()?;
        let mut dispatches = Vec::new();
        let descriptors = self.registry.descriptor_snapshot();
        for descriptor in descriptors.iter() {
            if !dispatch_selection::runner_can_dispatch(descriptor) {
                continue;
            }
            let load = self.tasks.runner_load(
                descriptor,
                self.current_step,
                self.load_plan.registry_generation,
            );
            let decision = decide_schedule(
                descriptor,
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
        descriptor: &RunnerDescriptor,
        decision: ScheduleDecision,
        lease_expires_at: Option<u64>,
    ) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
        dispatch_selection::claim_runner_work(self, descriptor, decision, lease_expires_at)
    }

    fn build_runner_dispatch(
        &mut self,
        descriptor: &RunnerDescriptor,
        executor_id: String,
        leased_tasks: Vec<(TaskLease, std::sync::Arc<Task>)>,
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
        self.scheduler_decisions = self.scheduler_decisions.saturating_add(1);
        if !self.load_plan.observability.detailed_scheduler_decisions {
            return;
        }
        if !self.events.is_enabled() && !self.traces.will_retain_next() {
            return;
        }
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
        if self.traces.will_retain_next()
            && let Some(span) = self.traces.record(
                format!("trace-scheduler-{}", descriptor.runner_id),
                "scheduler.decision",
                None,
                SpanStatus::Ok,
                attrs.clone(),
            )
        {
            attrs.insert("span_id".into(), ScalarValue::String(span.span_id));
        }
        if self.events.is_enabled() {
            self.events.record(
                RuntimeEventKind::Trace,
                "scheduler.decision",
                Some(descriptor.runner_id.clone()),
                attrs,
                None,
            );
        }
    }

    pub fn complete_runner_dispatch(
        &mut self,
        completion: RunnerCompletion,
    ) -> RuntimeResult<RunnerLoopReport> {
        completion_router::complete_runner_dispatch(self, completion)
    }

    /// Returns a dispatch that could not enter a bounded Host executor queue to Ready without
    /// losing its runner or leaving active leases behind.
    pub fn defer_runner_dispatch(&mut self, dispatch: RunnerDispatch) -> RuntimeResult<usize> {
        for lease in &dispatch.task_leases {
            self.tasks.ensure_active_lease(
                &lease.task_id,
                lease,
                self.current_step,
                "host_defer",
            )?;
        }
        for lease in &dispatch.task_leases {
            self.tasks.defer_leased(lease, self.current_step)?;
        }
        let deferred = dispatch.task_leases.len();
        if let executor::RunnerDispatchTarget::Sync(runner) = dispatch.target {
            self.registry.put_runner(runner);
        }
        Ok(deferred)
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
