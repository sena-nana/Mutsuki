use mutsuki_runtime_contracts::{
    ExecutionClass, RunnerDescriptor, RunnerPurity, RuntimeError, RuntimeEventKind, SpanStatus,
};

use crate::runner::{RunnerContext, RunnerLoopReport};
use crate::task_pool::RunnerLoad;
use crate::{RuntimeFailure, RuntimeResult};

use super::CoreRuntime;
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
        self.tasks.reclaim_expired_leases(self.current_step);
        let mut claimed = 0;
        let mut completed = 0;
        let descriptors = self.registry.descriptors();
        for descriptor in descriptors {
            let executor_id = format!("executor:{}", descriptor.runner_id);
            let leased_tasks = self.tasks.claim_ready_for_executor(
                &descriptor,
                executor_id.clone(),
                self.current_step,
                self.load_plan.registry_generation,
                1,
            );
            if leased_tasks.is_empty() {
                continue;
            }
            claimed += leased_tasks.len();
            if descriptor.purity == RunnerPurity::Committer && descriptor.runner_id == "core.kernel"
            {
                completed += self.process_kernel_tasks(&descriptor, leased_tasks)?;
                continue;
            }
            let (task_leases, tasks): (Vec<_>, Vec<_>) = leased_tasks.into_iter().unzip();
            let lease_id = task_leases[0].lease_id.clone();
            let runner = self
                .registry
                .take_runner(&descriptor.runner_id)
                .ok_or_else(|| {
                    RuntimeFailure::new(RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                        "runtime.runner_loop",
                        format!("runner.{}", descriptor.runner_id),
                    ))
                })?;
            let ctx = RunnerContext {
                registry_generation: self.load_plan.registry_generation,
                current_step: self.current_step,
                executor_id,
                task_lease_id: Some(lease_id),
            };
            let span = self.traces.record(
                format!("trace-runner-{}", descriptor.runner_id),
                "runner.step",
                None,
                SpanStatus::Ok,
                runner_attrs(&descriptor, &self.load_plan),
            );
            self.events.record(
                RuntimeEventKind::Trace,
                "trace.span",
                Some(descriptor.runner_id.clone()),
                trace_attrs(&span),
                None,
            );
            let completion = executor.execute(RunnerDispatch {
                runner,
                ctx,
                task_leases,
                tasks,
            });
            let results = completion.results;
            self.registry.put_runner(completion.runner);
            let task_leases = completion.task_leases;
            let results = results?;
            for result in results {
                let lease = task_leases
                    .iter()
                    .find(|lease| lease.task_id == result.task_id)
                    .ok_or_else(|| {
                        RuntimeFailure::new(RuntimeError::new(
                            mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                            "runtime.runner_loop",
                            format!("task.result.{}", result.task_id),
                        ))
                    })?;
                completed += self.route_result(&descriptor, lease, result)?;
            }
        }
        Ok(RunnerLoopReport {
            claimed_tasks: claimed,
            completed_tasks: completed,
        })
    }

    pub fn claim_ready_dispatches(
        &mut self,
        mut dispatch_limit: impl FnMut(&RunnerDescriptor, &RunnerLoad) -> usize,
        lease_expires_at: Option<u64>,
    ) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
        self.current_step += 1;
        self.tasks.reclaim_expired_leases(self.current_step);
        let mut claimed = 0;
        let mut completed = 0;
        let mut dispatches = Vec::new();
        let descriptors = self.registry.descriptors();
        for descriptor in descriptors {
            if descriptor.execution_class == ExecutionClass::Control
                && descriptor.runner_id != "core.kernel"
            {
                continue;
            }
            let load = self.tasks.runner_load(
                &descriptor,
                self.current_step,
                self.load_plan.registry_generation,
            );
            let limit = dispatch_limit(&descriptor, &load);
            if limit == 0 {
                continue;
            }
            let executor_id = format!("executor:{}", descriptor.runner_id);
            let leased_tasks = self.tasks.claim_ready_for_executor_with_expiry(
                &descriptor,
                executor_id.clone(),
                self.current_step,
                self.load_plan.registry_generation,
                limit,
                lease_expires_at,
            );
            if leased_tasks.is_empty() {
                continue;
            }
            claimed += leased_tasks.len();
            if descriptor.purity == RunnerPurity::Committer && descriptor.runner_id == "core.kernel"
            {
                completed += self.process_kernel_tasks(&descriptor, leased_tasks)?;
                continue;
            }
            let (task_leases, tasks): (Vec<_>, Vec<_>) = leased_tasks.into_iter().unzip();
            let lease_id = task_leases[0].lease_id.clone();
            let runner = self
                .registry
                .take_runner(&descriptor.runner_id)
                .ok_or_else(|| {
                    RuntimeFailure::new(RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                        "runtime.runner_loop",
                        format!("runner.{}", descriptor.runner_id),
                    ))
                })?;
            let ctx = RunnerContext {
                registry_generation: self.load_plan.registry_generation,
                current_step: self.current_step,
                executor_id,
                task_lease_id: Some(lease_id),
            };
            let span = self.traces.record(
                format!("trace-runner-{}", descriptor.runner_id),
                "runner.step",
                None,
                SpanStatus::Ok,
                runner_attrs(&descriptor, &self.load_plan),
            );
            self.events.record(
                RuntimeEventKind::Trace,
                "trace.span",
                Some(descriptor.runner_id.clone()),
                trace_attrs(&span),
                None,
            );
            dispatches.push(RunnerDispatch {
                runner,
                ctx,
                task_leases,
                tasks,
            });
        }
        Ok((
            RunnerLoopReport {
                claimed_tasks: claimed,
                completed_tasks: completed,
            },
            dispatches,
        ))
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
        let mut completed = 0;
        for result in results {
            let lease = task_leases
                .iter()
                .find(|lease| lease.task_id == result.task_id)
                .ok_or_else(|| {
                    RuntimeFailure::new(RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                        "runtime.runner_loop",
                        format!("task.result.{}", result.task_id),
                    ))
                })?;
            completed += self.route_result(&descriptor, lease, result)?;
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

    pub fn run_until_idle(&mut self, max_ticks: usize) -> RuntimeResult<RunnerLoopReport> {
        let mut aggregate = RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: 0,
        };
        for _ in 0..max_ticks {
            let report = self.tick_once()?;
            aggregate.claimed_tasks += report.claimed_tasks;
            aggregate.completed_tasks += report.completed_tasks;
            if self.tasks.ready_count() == 0 && self.tasks.running_count() == 0 {
                break;
            }
        }
        Ok(aggregate)
    }
}
