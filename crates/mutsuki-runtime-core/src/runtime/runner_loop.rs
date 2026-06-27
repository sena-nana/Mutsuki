use mutsuki_runtime_contracts::{RunnerPurity, RuntimeError, RuntimeEventKind, SpanStatus};

use crate::runner::{RunnerContext, RunnerLoopReport};
use crate::{RuntimeFailure, RuntimeResult};

use super::CoreRuntime;
use trace_metadata::{runner_attrs, trace_attrs};

mod kernel;
mod result_router;
mod trace_metadata;

impl CoreRuntime {
    pub fn tick_once(&mut self) -> RuntimeResult<RunnerLoopReport> {
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
            let mut runner = self
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
            let results = runner.step(ctx, tasks)?;
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
            self.registry.put_runner(runner);
        }
        Ok(RunnerLoopReport {
            claimed_tasks: claimed,
            completed_tasks: completed,
        })
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
