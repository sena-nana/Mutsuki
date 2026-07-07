use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{RuntimeError, RuntimeEventKind, ScalarValue, TaskLease};

use crate::RuntimeResult;

use super::CoreRuntime;

pub(super) fn reclaim_expired_task_leases(runtime: &mut CoreRuntime) {
    let reclaimed = runtime
        .tasks
        .reclaim_expired_task_leases(runtime.current_step);
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
            ScalarValue::Int(runtime.current_step as i64),
        );
        runtime.events.record(
            RuntimeEventKind::Task,
            "task.lease.expired",
            Some(lease.task_id.clone()),
            attrs,
            None,
        );
    }
}

pub(super) fn fail_runner_dispatch(
    runtime: &mut CoreRuntime,
    task_lease: &mutsuki_runtime_contracts::TaskLease,
    failure: RuntimeError,
) -> RuntimeResult<usize> {
    if runtime
        .tasks
        .fail(task_lease, runtime.current_step, failure.clone())
        .is_ok()
    {
        runtime.record_task_terminal_event(
            &task_lease.task_id,
            "task.failed",
            Some(failure.clone()),
        );
        runtime.wake_tasks_waiting_on(&task_lease.task_id)?;
        return Ok(1);
    }
    Ok(0)
}

pub(super) fn fail_runner_dispatches(
    runtime: &mut CoreRuntime,
    leases: &[TaskLease],
    failure: RuntimeError,
) -> RuntimeResult<usize> {
    let mut completed = 0;
    for lease in leases {
        completed += fail_runner_dispatch(runtime, lease, failure.clone())?;
    }
    Ok(completed)
}

pub(super) fn record_rejected_runner_result(
    runtime: &mut CoreRuntime,
    task_id: String,
    error: RuntimeError,
) {
    let mut attrs = BTreeMap::new();
    attrs.insert("error_code".into(), ScalarValue::String(error.code.clone()));
    attrs.insert(
        "error_route".into(),
        ScalarValue::String(error.route.clone()),
    );
    attrs.insert(
        "current_step".into(),
        ScalarValue::Int(runtime.current_step as i64),
    );
    runtime.events.record(
        RuntimeEventKind::Task,
        "task.result.rejected",
        Some(task_id),
        attrs,
        Some(error),
    );
}
