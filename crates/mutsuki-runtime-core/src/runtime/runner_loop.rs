use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, RunnerDescriptor, RunnerPurity, RuntimeError,
    RuntimeEventKind, ScalarValue, SpanStatus, Task, TaskLease,
};
use std::collections::BTreeMap;

use crate::RuntimeResult;
use crate::task_pool::{RunnerLoad, TASK_LEASE_TTL_STEPS};
use crate::{RunnerContext, RunnerLoopReport};

use super::CoreRuntime;
use super::ScheduleDecision;
use executor::{InlineRunnerExecutor, RunnerExecutor};
pub use executor::{RunnerCompletion, RunnerDispatch};
use trace_metadata::{runner_attrs, trace_attrs};

mod batch;
mod executor;
mod kernel;
mod result_router;
mod trace_metadata;

use batch::{build_work_batch, dispatch_batch_attrs, split_leased_tasks_by_resource_conflict};

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
            if !runner_can_dispatch(&descriptor) {
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
            if !runner_can_dispatch(&descriptor) {
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
        let decision = decision.clamp_to(descriptor.batch.max_batch_entries);
        self.record_scheduler_decision(&descriptor, &decision);
        if decision.dispatch_limit == 0 {
            return Ok((empty_runner_loop_report(), Vec::new()));
        }
        let executor_id = format!("executor:{}", descriptor.runner_id);
        let leased_tasks = self.tasks.claim_ready_for_executor_with_budget(
            &descriptor,
            executor_id.clone(),
            self.current_step,
            self.load_plan.registry_generation,
            decision.dispatch_limit,
            Some(&decision.budget),
            lease_expires_at,
        );
        if leased_tasks.is_empty() {
            return Ok((empty_runner_loop_report(), Vec::new()));
        }
        if descriptor.purity == RunnerPurity::Committer && descriptor.runner_id == "core.kernel" {
            let claimed_tasks = leased_tasks.len();
            let completed_tasks = self.process_kernel_tasks(&descriptor, leased_tasks)?;
            return Ok((
                RunnerLoopReport {
                    claimed_tasks,
                    completed_tasks,
                },
                Vec::new(),
            ));
        }
        let mut dispatch_groups = split_leased_tasks_by_resource_conflict(leased_tasks);
        let dispatch_group = dispatch_groups.remove(0);
        for deferred_group in dispatch_groups {
            for (lease, _task) in deferred_group {
                self.tasks.defer_leased(&lease, self.current_step)?;
            }
        }
        let claimed_tasks = dispatch_group.len();
        let dispatch = self.build_runner_dispatch(&descriptor, executor_id, dispatch_group)?;
        Ok((
            RunnerLoopReport {
                claimed_tasks,
                completed_tasks: 0,
            },
            vec![dispatch],
        ))
    }

    fn build_runner_dispatch(
        &mut self,
        descriptor: &RunnerDescriptor,
        executor_id: String,
        leased_tasks: Vec<(TaskLease, Task)>,
    ) -> RuntimeResult<RunnerDispatch> {
        let batch_id = format!(
            "batch-{}-{}-{}",
            self.current_step,
            descriptor.runner_id,
            leased_tasks
                .first()
                .map(|(_, task)| task.created_sequence)
                .unwrap_or_default()
        );
        let invocation_id = batch_id.clone();
        let trace_id = leased_tasks
            .first()
            .map(|(_, task)| dispatch_trace_id(task))
            .unwrap_or_else(|| format!("trace-batch-{batch_id}"));
        let task_leases: Vec<_> = leased_tasks
            .iter()
            .map(|(task_lease, _)| task_lease.clone())
            .collect();
        let batch = build_work_batch(self.current_step, &batch_id, descriptor, &leased_tasks);
        let mut attrs = runner_attrs(descriptor, &self.load_plan);
        attrs.extend(dispatch_batch_attrs(
            &batch,
            &leased_tasks,
            &task_leases,
            &executor_id,
        ));
        let runner = self
            .registry
            .take_runner(&descriptor.runner_id)
            .ok_or_else(|| {
                crate::runtime_failure(
                    mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                    "runtime.runner_loop",
                    format!("runner.{}", descriptor.runner_id),
                )
            })?;
        let ctx = RunnerContext::new(
            self.load_plan.registry_generation,
            self.current_step,
            executor_id,
            task_leases
                .iter()
                .map(|task_lease| task_lease.lease_id.clone())
                .collect::<Vec<_>>(),
            invocation_id,
        )
        .with_batch(batch_id.clone(), task_leases.len());
        let span = self
            .traces
            .record(trace_id, "runner.run_batch", None, SpanStatus::Ok, attrs);
        self.events.record(
            RuntimeEventKind::Trace,
            "trace.span",
            Some(descriptor.runner_id.clone()),
            trace_attrs(&span),
            None,
        );
        Ok(RunnerDispatch {
            runner,
            ctx,
            task_leases,
            batch,
        })
    }

    fn complete_inline_dispatch(&mut self, completion: RunnerCompletion) -> RuntimeResult<usize> {
        Ok(self.complete_runner_dispatch(completion)?.completed_tasks)
    }

    fn record_scheduler_decision(
        &mut self,
        descriptor: &RunnerDescriptor,
        decision: &ScheduleDecision,
    ) {
        let mut attrs = BTreeMap::from([
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
            SpanStatus::Ok,
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
        let descriptor = completion.runner.descriptor().clone();
        let result = completion.result;
        self.registry.put_runner(completion.runner);
        let result = match result {
            Ok(result) => result,
            Err(failure) => {
                let completed =
                    self.fail_runner_dispatches(&completion.task_leases, failure.error().clone())?;
                return Ok(RunnerLoopReport {
                    claimed_tasks: 0,
                    completed_tasks: completed,
                });
            }
        };
        if result.batch_id != completion.batch_id {
            let failure = batch_claim_conflict(format!("batch.result.{}", result.batch_id));
            let completed = self.fail_runner_dispatches(&completion.task_leases, failure)?;
            return Ok(RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: completed,
            });
        }
        let completed =
            self.route_completion_batch(&descriptor, &completion.task_leases, result)?;
        Ok(RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: completed,
        })
    }

    fn route_completion_batch(
        &mut self,
        descriptor: &RunnerDescriptor,
        leases: &[TaskLease],
        batch: CompletionBatch,
    ) -> RuntimeResult<usize> {
        let mut leases_by_task = BTreeMap::new();
        for lease in leases {
            leases_by_task.insert(lease.task_id.clone(), lease.clone());
        }
        let mut seen_entries = BTreeMap::new();
        for completion in &batch.results {
            if seen_entries
                .insert(completion.entry_id.clone(), completion.task_id.clone())
                .is_some()
                || !leases_by_task.contains_key(&completion.task_id)
            {
                return self.fail_runner_dispatches(
                    leases,
                    batch_claim_conflict(format!("batch.entry.{}", completion.entry_id)),
                );
            }
        }
        let mut completed = 0;
        for lease in leases {
            let Some(completion) = batch
                .results
                .iter()
                .find(|completion| completion.task_id == lease.task_id)
            else {
                completed += self.fail_runner_dispatch(
                    lease,
                    batch_claim_conflict(format!("batch.missing.{}", lease.task_id)),
                )?;
                continue;
            };
            completed += self.route_entry_completion(descriptor, lease, completion.clone())?;
        }
        Ok(completed)
    }

    fn route_entry_completion(
        &mut self,
        descriptor: &RunnerDescriptor,
        lease: &TaskLease,
        completion: EntryCompletion,
    ) -> RuntimeResult<usize> {
        if let Some(error) = completion.error {
            return self.fail_runner_dispatch(lease, error);
        }
        let Some(result) = completion.result else {
            return self.fail_runner_dispatch(
                lease,
                batch_claim_conflict(format!("batch.entry.empty.{}", completion.entry_id)),
            );
        };
        if result.task_id != lease.task_id || completion.task_id != lease.task_id {
            return self.fail_runner_dispatch(
                lease,
                batch_claim_conflict(format!("task.result.{}", result.task_id)),
            );
        }
        match self.route_result(descriptor, lease, result) {
            Ok(count) => Ok(count),
            Err(failure) if is_stale_completion_conflict(failure.error()) => {
                self.record_rejected_runner_result(lease.task_id.clone(), failure.error().clone());
                Ok(0)
            }
            Err(failure) => Err(failure),
        }
    }

    fn fail_runner_dispatch(
        &mut self,
        task_lease: &mutsuki_runtime_contracts::TaskLease,
        failure: RuntimeError,
    ) -> RuntimeResult<usize> {
        if self
            .tasks
            .fail(task_lease, self.current_step, failure.clone())
            .is_ok()
        {
            self.record_task_terminal_event(
                &task_lease.task_id,
                "task.failed",
                Some(failure.clone()),
            );
            self.wake_tasks_waiting_on(&task_lease.task_id)?;
            return Ok(1);
        }
        Ok(0)
    }

    fn fail_runner_dispatches(
        &mut self,
        leases: &[TaskLease],
        failure: RuntimeError,
    ) -> RuntimeResult<usize> {
        let mut completed = 0;
        for lease in leases {
            completed += self.fail_runner_dispatch(lease, failure.clone())?;
        }
        Ok(completed)
    }

    fn reclaim_expired_task_leases(&mut self) {
        let reclaimed = self.tasks.reclaim_expired_task_leases(self.current_step);
        for lease in &reclaimed {
            let mut attrs = BTreeMap::new();
            attrs.insert(
                "lease_id".into(),
                ScalarValue::String(lease.lease_id.clone()),
            );
            attrs.insert(
                "runner_id".into(),
                ScalarValue::String(lease.runner_id.clone()),
            );
            attrs.insert(
                "executor_id".into(),
                ScalarValue::String(lease.executor_id.clone()),
            );
            attrs.insert(
                "registry_generation".into(),
                ScalarValue::Int(lease.registry_generation as i64),
            );
            attrs.insert(
                "acquired_at_step".into(),
                ScalarValue::Int(lease.acquired_at_step as i64),
            );
            if let Some(expires_at_step) = lease.expires_at_step {
                attrs.insert(
                    "expires_at_step".into(),
                    ScalarValue::Int(expires_at_step as i64),
                );
            }
            attrs.insert(
                "current_step".into(),
                ScalarValue::Int(self.current_step as i64),
            );
            self.events.record(
                RuntimeEventKind::Task,
                "task.lease.expired",
                Some(lease.task_id.clone()),
                attrs,
                None,
            );
        }
    }

    fn record_rejected_runner_result(&mut self, task_id: String, error: RuntimeError) {
        let mut attrs = BTreeMap::new();
        attrs.insert("error_code".into(), ScalarValue::String(error.code.clone()));
        attrs.insert(
            "error_route".into(),
            ScalarValue::String(error.route.clone()),
        );
        attrs.insert(
            "current_step".into(),
            ScalarValue::Int(self.current_step as i64),
        );
        self.events.record(
            RuntimeEventKind::Task,
            "task.result.rejected",
            Some(task_id),
            attrs,
            Some(error),
        );
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

fn is_stale_completion_conflict(error: &RuntimeError) -> bool {
    error.code == mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT
        && error.route.starts_with("task.route.")
}

fn runner_can_dispatch(descriptor: &RunnerDescriptor) -> bool {
    descriptor.execution_class != ExecutionClass::Control || descriptor.runner_id == "core.kernel"
}

fn dispatch_trace_id(task: &mutsuki_runtime_contracts::Task) -> String {
    task.trace_id
        .clone()
        .unwrap_or_else(|| format!("trace-task-{}", task.task_id))
}

fn batch_claim_conflict(route: String) -> RuntimeError {
    crate::runtime_error(
        mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
        "runtime.runner_loop",
        route,
    )
}

fn empty_runner_loop_report() -> RunnerLoopReport {
    RunnerLoopReport {
        claimed_tasks: 0,
        completed_tasks: 0,
    }
}
