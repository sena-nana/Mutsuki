use mutsuki_runtime_contracts::{ExecutionClass, RunnerDescriptor, RunnerPurity};

use crate::RuntimeResult;

use super::{CoreRuntime, RunnerDispatch, RunnerLoopReport, ScheduleDecision, batch};

pub(super) fn runner_can_dispatch(descriptor: &RunnerDescriptor) -> bool {
    descriptor.execution_class != ExecutionClass::Control || descriptor.runner_id == "core.kernel"
}

pub(super) fn claim_runner_work(
    runtime: &mut CoreRuntime,
    descriptor: RunnerDescriptor,
    decision: ScheduleDecision,
    lease_expires_at: Option<u64>,
) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
    let decision = decision.clamp_to(descriptor.batch.max_batch_entries);
    runtime.record_scheduler_decision(&descriptor, &decision);
    if decision.dispatch_limit == 0 {
        return Ok((empty_runner_loop_report(), Vec::new()));
    }
    let executor_id = format!("executor:{}", descriptor.runner_id);
    let leased_tasks = runtime.tasks.claim_ready_for_executor_with_budget(
        &descriptor,
        executor_id.clone(),
        runtime.current_step,
        runtime.load_plan.registry_generation,
        decision.dispatch_limit,
        Some(&decision.budget),
        lease_expires_at,
    );
    if leased_tasks.is_empty() {
        return Ok((empty_runner_loop_report(), Vec::new()));
    }
    if descriptor.purity == RunnerPurity::Committer && descriptor.runner_id == "core.kernel" {
        let claimed_tasks = leased_tasks.len();
        let completed_tasks = runtime.process_kernel_tasks(&descriptor, leased_tasks)?;
        return Ok((
            RunnerLoopReport {
                claimed_tasks,
                completed_tasks,
            },
            Vec::new(),
        ));
    }
    let mut dispatch_groups = batch::split_leased_tasks_by_resource_conflict(leased_tasks);
    let dispatch_group = dispatch_groups.remove(0);
    for deferred_group in dispatch_groups {
        for (lease, _task) in deferred_group {
            runtime.tasks.defer_leased(&lease, runtime.current_step)?;
        }
    }
    let claimed_tasks = dispatch_group.len();
    let dispatch = runtime.build_runner_dispatch(&descriptor, executor_id, dispatch_group)?;
    Ok((
        RunnerLoopReport {
            claimed_tasks,
            completed_tasks: 0,
        },
        vec![dispatch],
    ))
}

fn empty_runner_loop_report() -> RunnerLoopReport {
    RunnerLoopReport {
        claimed_tasks: 0,
        completed_tasks: 0,
    }
}
