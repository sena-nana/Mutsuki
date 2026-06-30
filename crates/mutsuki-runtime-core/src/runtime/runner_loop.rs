use mutsuki_runtime_contracts::{
    ExecutionClass, RunnerDescriptor, RunnerPurity, RuntimeError, RuntimeEventKind, ScalarValue,
    SpanStatus,
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

mod executor;
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
            if !runner_can_dispatch(&descriptor) {
                continue;
            }
            let load = self.tasks.runner_load(
                &descriptor,
                self.current_step,
                self.load_plan.registry_generation,
            );
            let decision = ScheduleDecision::new("core.inline", 1, "inline.default")
                .clamp_to(load.queued_count);
            let (report, dispatch) = self.claim_runner_work(
                descriptor,
                decision,
                Some(self.current_step + TASK_LEASE_TTL_STEPS),
            )?;
            loop_report.claimed_tasks += report.claimed_tasks;
            loop_report.completed_tasks += report.completed_tasks;
            if let Some(dispatch) = dispatch {
                loop_report.completed_tasks +=
                    self.complete_inline_dispatch(executor.execute(dispatch))?;
            }
        }
        Ok(loop_report)
    }

    pub fn claim_ready_dispatches(
        &mut self,
        mut decide_schedule: impl FnMut(&RunnerDescriptor, &RunnerLoad, u64, u64) -> ScheduleDecision,
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
            );
            let (report, dispatch) =
                self.claim_runner_work(descriptor, decision, lease_expires_at)?;
            loop_report.claimed_tasks += report.claimed_tasks;
            loop_report.completed_tasks += report.completed_tasks;
            if let Some(dispatch) = dispatch {
                dispatches.push(dispatch);
            }
        }
        Ok((loop_report, dispatches))
    }

    fn claim_runner_work(
        &mut self,
        descriptor: RunnerDescriptor,
        decision: ScheduleDecision,
        lease_expires_at: Option<u64>,
    ) -> RuntimeResult<(RunnerLoopReport, Option<RunnerDispatch>)> {
        self.record_scheduler_decision(&descriptor, &decision);
        if decision.dispatch_limit == 0 {
            return Ok((empty_runner_loop_report(), None));
        }
        let executor_id = format!("executor:{}", descriptor.runner_id);
        let leased_tasks = self.tasks.claim_ready_for_executor_with_expiry(
            &descriptor,
            executor_id.clone(),
            self.current_step,
            self.load_plan.registry_generation,
            decision.dispatch_limit,
            lease_expires_at,
        );
        if leased_tasks.is_empty() {
            return Ok((empty_runner_loop_report(), None));
        }
        let claimed_tasks = leased_tasks.len();
        if descriptor.purity == RunnerPurity::Committer && descriptor.runner_id == "core.kernel" {
            let completed_tasks = self.process_kernel_tasks(&descriptor, leased_tasks)?;
            return Ok((
                RunnerLoopReport {
                    claimed_tasks,
                    completed_tasks,
                },
                None,
            ));
        }
        Ok((
            RunnerLoopReport {
                claimed_tasks,
                completed_tasks: 0,
            },
            Some(self.build_runner_dispatch(&descriptor, executor_id, leased_tasks)?),
        ))
    }

    fn build_runner_dispatch(
        &mut self,
        descriptor: &RunnerDescriptor,
        executor_id: String,
        leased_tasks: Vec<(
            mutsuki_runtime_contracts::TaskLease,
            mutsuki_runtime_contracts::Task,
        )>,
    ) -> RuntimeResult<RunnerDispatch> {
        let (task_leases, tasks): (Vec<_>, Vec<_>) = leased_tasks.into_iter().unzip();
        let lease_id = task_leases[0].lease_id.clone();
        let invocation_id = lease_id.clone();
        let trace_id = dispatch_trace_id(&tasks);
        let mut attrs = runner_attrs(descriptor, &self.load_plan);
        attrs.extend(dispatch_task_attrs(&tasks, &task_leases, &executor_id));
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
            Some(lease_id),
            invocation_id,
        );
        let span = self
            .traces
            .record(trace_id, "runner.step", None, SpanStatus::Ok, attrs);
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
            tasks,
        })
    }

    fn complete_inline_dispatch(&mut self, completion: RunnerCompletion) -> RuntimeResult<usize> {
        let descriptor = completion.runner.descriptor().clone();
        let results = completion.results;
        self.registry.put_runner(completion.runner);
        let task_leases = completion.task_leases;
        let results = results?;
        let result_leases = match validate_dispatch_results(&task_leases, &results) {
            Ok(result_leases) => result_leases,
            Err(failure) => {
                return self.fail_runner_dispatch(&task_leases, failure.error().clone());
            }
        };
        let mut completed = 0;
        for result in results {
            let lease = result_leases
                .get(&result.task_id)
                .expect("validated runner result has a matching task lease");
            completed += self.route_result(&descriptor, lease, result)?;
        }
        Ok(completed)
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
        let results = completion.results;
        self.registry.put_runner(completion.runner);
        let task_leases = completion.task_leases;
        let results = match results {
            Ok(results) => results,
            Err(failure) => {
                let completed = self.fail_runner_dispatch(&task_leases, failure.error().clone())?;
                return Ok(RunnerLoopReport {
                    claimed_tasks: 0,
                    completed_tasks: completed,
                });
            }
        };
        let result_leases = match validate_dispatch_results(&task_leases, &results) {
            Ok(result_leases) => result_leases,
            Err(failure) => {
                let completed = self.fail_runner_dispatch(&task_leases, failure.error().clone())?;
                return Ok(RunnerLoopReport {
                    claimed_tasks: 0,
                    completed_tasks: completed,
                });
            }
        };
        let mut completed = 0;
        for result in results {
            let lease = result_leases
                .get(&result.task_id)
                .expect("validated runner result has a matching task lease");
            match self.route_result(&descriptor, lease, result) {
                Ok(count) => completed += count,
                Err(failure) if is_stale_completion_conflict(failure.error()) => {
                    self.record_rejected_runner_result(
                        lease.task_id.clone(),
                        failure.error().clone(),
                    );
                }
                Err(failure) => return Err(failure),
            }
        }
        Ok(RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: completed,
        })
    }

    fn fail_runner_dispatch(
        &mut self,
        task_leases: &[mutsuki_runtime_contracts::TaskLease],
        failure: RuntimeError,
    ) -> RuntimeResult<usize> {
        let mut completed = 0;
        for lease in task_leases {
            if self
                .tasks
                .fail(lease, self.current_step, failure.clone())
                .is_ok()
            {
                self.record_task_terminal_event(
                    &lease.task_id,
                    "task.failed",
                    Some(failure.clone()),
                );
                self.wake_tasks_waiting_on(&lease.task_id)?;
                completed += 1;
            }
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

fn validate_dispatch_results<'a>(
    task_leases: &'a [mutsuki_runtime_contracts::TaskLease],
    results: &[mutsuki_runtime_contracts::RunnerResult],
) -> RuntimeResult<BTreeMap<String, &'a mutsuki_runtime_contracts::TaskLease>> {
    let mut pending_leases = task_leases
        .iter()
        .map(|lease| (lease.task_id.as_str(), lease))
        .collect::<BTreeMap<_, _>>();
    let mut result_leases = BTreeMap::new();
    for result in results {
        if result_leases.contains_key(&result.task_id) {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                "runtime.runner_loop",
                format!("task.result.duplicate.{}", result.task_id),
            ));
        }
        let Some(lease) = pending_leases.remove(result.task_id.as_str()) else {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                "runtime.runner_loop",
                format!("task.result.{}", result.task_id),
            ));
        };
        result_leases.insert(result.task_id.clone(), lease);
    }
    if let Some(task_id) = pending_leases.keys().next() {
        return Err(crate::runtime_failure(
            mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
            "runtime.runner_loop",
            format!("task.result.missing.{task_id}"),
        ));
    }
    Ok(result_leases)
}

fn runner_can_dispatch(descriptor: &RunnerDescriptor) -> bool {
    descriptor.execution_class != ExecutionClass::Control || descriptor.runner_id == "core.kernel"
}

fn dispatch_trace_id(tasks: &[mutsuki_runtime_contracts::Task]) -> String {
    let Some(task) = tasks.first() else {
        return "trace-runner-empty".into();
    };
    task.trace_id
        .clone()
        .unwrap_or_else(|| format!("trace-task-{}", task.task_id))
}

fn dispatch_task_attrs(
    tasks: &[mutsuki_runtime_contracts::Task],
    task_leases: &[mutsuki_runtime_contracts::TaskLease],
    executor_id: &str,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::from([
        (
            "executor_id".into(),
            ScalarValue::String(executor_id.into()),
        ),
        ("task_count".into(), ScalarValue::Int(tasks.len() as i64)),
    ]);
    if let Some(task) = tasks.first() {
        attrs.insert("task_id".into(), ScalarValue::String(task.task_id.clone()));
        if let Some(correlation_id) = &task.correlation_id {
            attrs.insert(
                "correlation_id".into(),
                ScalarValue::String(correlation_id.clone()),
            );
        }
    }
    if let Some(lease) = task_leases.first() {
        attrs.insert(
            "task_lease_id".into(),
            ScalarValue::String(lease.lease_id.clone()),
        );
    }
    attrs
}

fn empty_runner_loop_report() -> RunnerLoopReport {
    RunnerLoopReport {
        claimed_tasks: 0,
        completed_tasks: 0,
    }
}
