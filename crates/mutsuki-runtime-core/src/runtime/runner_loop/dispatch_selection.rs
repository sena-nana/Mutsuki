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
        .min(descriptor.concurrency.max_inflight_batches())
        .min(
            runtime
                .registry
                .available_dispatch_instances(&descriptor.runner_id),
        );
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
    let mut dispatch_group = dispatch_groups.remove(0);
    for deferred_group in dispatch_groups {
        for (lease, _task) in deferred_group {
            runtime.tasks.defer_leased(&lease, runtime.current_step)?;
        }
    }
    // The group is split in place. Copying each chunk out first would duplicate every lease and
    // bump every task refcount for tasks that are about to be handed straight to a dispatch.
    let chunk_size = descriptor.batch.max_batch_entries.max(1);
    let dispatched_tasks = dispatch_group
        .len()
        .min(chunk_size.saturating_mul(max_batches));
    for (lease, _task) in dispatch_group.split_off(dispatched_tasks) {
        runtime.tasks.defer_leased(&lease, runtime.current_step)?;
    }
    let mut dispatches = Vec::with_capacity(dispatched_tasks.div_ceil(chunk_size));
    let mut remaining = dispatch_group;
    while !remaining.is_empty() {
        let tail = remaining.split_off(remaining.len().min(chunk_size));
        dispatches.push(runtime.build_runner_dispatch(
            descriptor,
            executor_id.clone(),
            remaining,
        )?);
        remaining = tail;
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
