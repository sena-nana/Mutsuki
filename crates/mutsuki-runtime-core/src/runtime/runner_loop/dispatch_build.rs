use mutsuki_runtime_contracts::{RuntimeEventKind, SpanStatus, Task, TaskLease};

use crate::RunnerContext;

use super::batch::{build_work_batch, dispatch_batch_attrs};
use super::trace_metadata::{runner_attrs, trace_attrs};
use super::{CoreRuntime, RunnerDispatch};

pub(super) fn build_runner_dispatch(
    runtime: &mut CoreRuntime,
    descriptor: &mutsuki_runtime_contracts::RunnerDescriptor,
    executor_id: String,
    leased_tasks: Vec<(TaskLease, Task)>,
) -> crate::RuntimeResult<RunnerDispatch> {
    let batch_id = format!(
        "batch-{}-{}-{}",
        runtime.current_step,
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
    let batch = build_work_batch(runtime.current_step, &batch_id, descriptor, &leased_tasks);
    let mut attrs = runner_attrs(descriptor, &runtime.load_plan);
    attrs.extend(dispatch_batch_attrs(
        descriptor,
        &batch,
        &leased_tasks,
        &task_leases,
        &executor_id,
    ));
    let runner = runtime
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
        runtime.load_plan.registry_generation,
        runtime.current_step,
        executor_id,
        task_leases
            .iter()
            .map(|task_lease| task_lease.lease_id.clone())
            .collect::<Vec<_>>(),
        invocation_id,
    )
    .with_batch(batch_id.clone(), task_leases.len());
    let span = runtime
        .traces
        .record(trace_id, "runner.run_batch", None, SpanStatus::Ok, attrs);
    runtime.events.record(
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

fn dispatch_trace_id(task: &mutsuki_runtime_contracts::Task) -> String {
    task.trace_id
        .clone()
        .unwrap_or_else(|| format!("trace-task-{}", task.task_id))
}
