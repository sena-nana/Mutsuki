use mutsuki_runtime_contracts::{ExecutionClass, RunnerDescriptor, RunnerPurity};

use crate::RuntimeResult;

use super::{CoreRuntime, RunnerDispatch, RunnerLoopReport, ScheduleDecision, batch};

pub(super) fn runner_can_dispatch(descriptor: &RunnerDescriptor) -> bool {
    descriptor.execution_class != ExecutionClass::Control || descriptor.runner_id == "core.kernel"
}

pub(super) fn claim_runner_work(
    runtime: &mut CoreRuntime,
    descriptor: &RunnerDescriptor,
    decision: ScheduleDecision,
    lease_expires_at: Option<u64>,
) -> RuntimeResult<(RunnerLoopReport, Vec<RunnerDispatch>)> {
    let max_batches = decision
        .budget
        .max_batches
        .min(descriptor.concurrency.max_inflight_batches());
    let decision = decision
        .clamp_to(
            descriptor
                .batch
                .max_batch_entries
                .saturating_mul(max_batches),
        )
        .clamp_batches(max_batches);
    runtime.record_scheduler_decision(descriptor, &decision);
    if decision.dispatch_limit == 0 {
        return Ok((empty_runner_loop_report(), Vec::new()));
    }
    let executor_id = format!("executor:{}", descriptor.runner_id);
    let leased_tasks = runtime.tasks.claim_ready_for_executor_shared_with_budget(
        descriptor,
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
        let completed_tasks = runtime.process_kernel_tasks(descriptor, leased_tasks)?;
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
    let mut chunks = dispatch_group
        .chunks(descriptor.batch.max_batch_entries)
        .take(max_batches)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();
    let dispatched_tasks: usize = chunks.iter().map(Vec::len).sum();
    let mut deferred_tail = dispatch_group.into_iter().skip(dispatched_tasks);
    for (lease, _task) in deferred_tail.by_ref() {
        runtime.tasks.defer_leased(&lease, runtime.current_step)?;
    }
    let mut dispatches = Vec::with_capacity(chunks.len());
    for chunk in chunks.drain(..) {
        dispatches.push(runtime.build_runner_dispatch(descriptor, executor_id.clone(), chunk)?);
    }
    Ok((
        RunnerLoopReport {
            claimed_tasks: dispatched_tasks,
            completed_tasks: 0,
        },
        dispatches,
    ))
}

fn empty_runner_loop_report() -> RunnerLoopReport {
    RunnerLoopReport {
        claimed_tasks: 0,
        completed_tasks: 0,
    }
}
