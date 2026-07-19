use mutsuki_runtime_contracts::{RuntimeEvent, RuntimeEventKind, SpanStatus, Task, TaskLease};

use crate::RunnerContext;

use super::batch::{build_work_batch, dispatch_batch_attrs};
use super::trace_metadata::{runner_attrs, trace_attrs};
use super::{CoreRuntime, RunnerDispatch, RunnerDispatchTarget};

pub(super) fn build_runner_dispatch(
    runtime: &mut CoreRuntime,
    descriptor: &mutsuki_runtime_contracts::RunnerDescriptor,
    executor_id: String,
    leased_tasks: Vec<(TaskLease, std::sync::Arc<Task>)>,
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
    let task_leases: Vec<_> = leased_tasks
        .iter()
        .map(|(task_lease, _)| task_lease.clone())
        .collect();
    let trace_id = leased_tasks
        .first()
        .map(|(_, task)| dispatch_trace_id(task))
        .unwrap_or_else(|| format!("trace-batch-{batch_id}"));
    let batch = build_work_batch(runtime.current_step, &batch_id, descriptor, leased_tasks);
    let span =
        if runtime.load_plan.observability.dispatch_spans && runtime.traces.will_retain_next() {
            let mut attrs = runner_attrs(descriptor, &runtime.load_plan);
            attrs.extend(dispatch_batch_attrs(
                descriptor,
                &batch,
                &task_leases,
                &executor_id,
            ));
            runtime
                .traces
                .record(trace_id, "runner.run_batch", None, SpanStatus::Ok, attrs)
        } else {
            None
        };
    let target = if let Some(handler) = runtime.registry.async_handler(&descriptor.runner_id) {
        RunnerDispatchTarget::Async(handler)
    } else {
        RunnerDispatchTarget::Sync(
            runtime
                .registry
                .take_runner(&descriptor.runner_id)
                .ok_or_else(|| {
                    crate::runtime_failure(
                        mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                        "runtime.runner_loop",
                        format!("runner.{}", descriptor.runner_id),
                    )
                })?,
        )
    };
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
    for lease in &task_leases {
        runtime.events.record_with(|sequence| RuntimeEvent {
            sequence,
            kind: RuntimeEventKind::Task,
            name: "task.started".into(),
            subject_id: Some(lease.task_id.clone()),
            attributes: std::collections::BTreeMap::from([
                (
                    "lease_id".into(),
                    mutsuki_runtime_contracts::ScalarValue::String(lease.lease_id.clone()),
                ),
                (
                    "runner_id".into(),
                    mutsuki_runtime_contracts::ScalarValue::String(lease.runner_id.clone()),
                ),
                (
                    "registry_generation".into(),
                    mutsuki_runtime_contracts::ScalarValue::Int(lease.registry_generation as i64),
                ),
            ]),
            error: None,
        });
    }
    if let Some(span) = span
        && runtime.events.is_enabled()
    {
        runtime.events.record(
            RuntimeEventKind::Trace,
            "trace.span",
            Some(descriptor.runner_id.clone()),
            trace_attrs(&span),
            None,
        );
    }
    Ok(RunnerDispatch {
        target,
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
