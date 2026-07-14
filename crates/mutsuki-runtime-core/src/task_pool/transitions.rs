use mutsuki_runtime_contracts::{
    ERR_TASK_CLAIM_CONFLICT, RuntimeError, ScalarValue, TaskLease, TaskStatus,
};

use crate::{RuntimeFailure, RuntimeResult};

use super::{TaskPool, TaskRecord};

pub(super) fn complete(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<()> {
    {
        let record = task_pool.leased_record_mut(lease, current_step, "complete")?;
        mark_terminal_record(record, TaskStatus::Completed, None);
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Completed));
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn fail(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
    failure: RuntimeError,
) -> RuntimeResult<()> {
    {
        let record = task_pool.leased_record_mut(lease, current_step, "fail")?;
        mark_terminal_record(record, TaskStatus::Failed, Some(failure));
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Failed));
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn wait(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
    ready_at_step: Option<u64>,
) -> RuntimeResult<()> {
    {
        let record = task_pool.leased_record_mut(lease, current_step, "wait")?;
        record.status = TaskStatus::Waiting;
        record.task.ready_at_step = ready_at_step;
        release_record_lease(record);
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Waiting));
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn defer_leased(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<()> {
    {
        let record = task_pool.leased_record_mut(lease, current_step, "defer")?;
        record.status = TaskStatus::Ready;
        record.ready_since_step = current_step;
        release_record_lease(record);
        clear_record_owner(record);
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Ready));
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn block(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<()> {
    {
        let record = task_pool.leased_record_mut(lease, current_step, "block")?;
        record.status = TaskStatus::Blocked;
        release_record_lease(record);
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Blocked));
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn wake(
    task_pool: &mut TaskPool,
    task_id: &str,
    current_step: u64,
) -> RuntimeResult<()> {
    let previous_status = {
        let record = task_pool.record_mut(task_id)?;
        if !matches!(record.status, TaskStatus::Waiting | TaskStatus::Blocked) {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.wake.{task_id}"),
            ));
        }
        let previous_status = record.status.clone();
        record.status = TaskStatus::Ready;
        record.ready_since_step = current_step;
        release_record_lease(record);
        previous_status
    };
    task_pool
        .statistics
        .record_status_transition(Some(&previous_status), Some(&TaskStatus::Ready));
    crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
    Ok(())
}

pub(super) fn wake_due_tasks(task_pool: &mut TaskPool, current_step: u64) -> Vec<(String, u64)> {
    let due_tasks: Vec<_> = task_pool
        .tasks
        .values()
        .filter_map(|record| {
            let ready_at_step = record.task.ready_at_step?;
            if matches!(record.status, TaskStatus::Waiting | TaskStatus::Blocked)
                && ready_at_step <= current_step
            {
                Some((record.task.task_id.clone(), ready_at_step))
            } else {
                None
            }
        })
        .collect();
    for (task_id, _) in &due_tasks {
        if let Some(record) = task_pool.tasks.get_mut(task_id) {
            let previous_status = record.status.clone();
            record.status = TaskStatus::Ready;
            record.ready_since_step = current_step;
            release_record_lease(record);
            task_pool
                .statistics
                .record_status_transition(Some(&previous_status), Some(&TaskStatus::Ready));
        }
        crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
    }
    due_tasks
}

pub(super) fn reject_ready(
    task_pool: &mut TaskPool,
    task_id: &str,
    failure: RuntimeError,
) -> RuntimeResult<()> {
    {
        let record = task_pool.record_mut(task_id)?;
        if record.status != TaskStatus::Ready {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.reject.{task_id}"),
            ));
        }
        record.status = TaskStatus::Failed;
        record.failure = Some(failure);
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Ready), Some(&TaskStatus::Failed));
    Ok(())
}

pub(super) fn cancel_running_invocation(
    task_pool: &mut TaskPool,
    runner_id: &str,
    invocation_id: &str,
    current_step: u64,
) -> usize {
    let mut cancelled = 0;
    for record in task_pool.tasks.values_mut() {
        if record.status != TaskStatus::Running || record.claimed_by.as_deref() != Some(runner_id) {
            continue;
        }
        if record
            .lease
            .as_ref()
            .is_some_and(|lease| lease.lease_id == invocation_id)
        {
            if let Some(lease) = record.lease.clone() {
                record_attempt_finished_value(&mut task_pool.statistics, &lease, current_step);
            }
            record.status = TaskStatus::Ready;
            task_pool
                .statistics
                .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Ready));
            record.ready_since_step = current_step;
            release_record_lease(record);
            cancelled = 1;
            break;
        }
    }
    cancelled
}

pub(super) fn cancel_task(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<()> {
    {
        let record = task_pool.leased_record_mut(lease, current_step, "cancel")?;
        mark_terminal_record(record, TaskStatus::Cancelled, None);
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Cancelled));
    record_attempt_finished(task_pool, lease, current_step);
    crate::task_pool::awaits::remove_waits_for_parent(task_pool, &lease.task_id);
    Ok(())
}

pub(super) fn terminal_by_core(
    task_pool: &mut TaskPool,
    task_id: &str,
    status: TaskStatus,
    failure: Option<RuntimeError>,
    action: &str,
    current_step: u64,
) -> RuntimeResult<()> {
    let (active_lease, previous_status) = {
        let record = task_pool.record_mut(task_id)?;
        if is_terminal_status(&record.status) {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.{action}.{task_id}"),
            ));
        }
        let active_lease = record.lease.clone();
        let previous_status = record.status.clone();
        mark_terminal_record(record, status.clone(), failure);
        (active_lease, previous_status)
    };
    task_pool
        .statistics
        .record_status_transition(Some(&previous_status), Some(&status));
    if let Some(lease) = active_lease {
        record_attempt_finished(task_pool, &lease, current_step);
    }
    crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
    Ok(())
}

pub(super) fn ensure_active_lease(
    task_pool: &TaskPool,
    task_id: &str,
    lease: &TaskLease,
    current_step: u64,
    action: &str,
) -> RuntimeResult<()> {
    validate_record_lease(task_pool.record(task_id)?, lease, current_step, action)
}

pub(super) fn reclaim_expired_task_leases(
    task_pool: &mut TaskPool,
    current_step: u64,
) -> Vec<TaskLease> {
    let mut reclaimed = Vec::new();
    for record in task_pool.tasks.values_mut() {
        if record.status != TaskStatus::Running {
            continue;
        }
        if record
            .lease
            .as_ref()
            .is_some_and(|lease| task_lease_expired(lease, current_step))
        {
            if let Some(lease) = record.lease.clone() {
                reclaimed.push(lease);
            }
            record.status = TaskStatus::Ready;
            task_pool
                .statistics
                .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Ready));
            record.ready_since_step = current_step;
            release_record_lease(record);
        }
    }
    for lease in &reclaimed {
        record_attempt_finished(task_pool, lease, current_step);
    }
    reclaimed
}

pub(super) fn abort_all(
    task_pool: &mut TaskPool,
    current_step: u64,
    failure: RuntimeError,
) -> Vec<String> {
    let mut aborted = Vec::new();
    let mut finished_leases = Vec::new();
    for record in task_pool.tasks.values_mut() {
        if is_terminal_status(&record.status) {
            continue;
        }
        if let Some(lease) = record.lease.clone() {
            finished_leases.push(lease);
        }
        aborted.push(record.task.task_id.clone());
        let previous_status = record.status.clone();
        mark_terminal_record(record, TaskStatus::Cancelled, Some(failure.clone()));
        task_pool
            .statistics
            .record_status_transition(Some(&previous_status), Some(&TaskStatus::Cancelled));
    }
    for lease in &finished_leases {
        record_attempt_finished(task_pool, lease, current_step);
    }
    aborted.sort();
    for task_id in &aborted {
        crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
    }
    aborted
}

pub(super) fn record_attempt_finished(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) {
    record_attempt_finished_value(&mut task_pool.statistics, lease, current_step);
}

fn record_attempt_finished_value(
    statistics: &mut super::TaskPoolStatistics,
    lease: &TaskLease,
    current_step: u64,
) {
    let elapsed = current_step
        .saturating_sub(lease.acquired_at_step)
        .saturating_add(1);
    statistics.cumulative_execution_steps = statistics
        .cumulative_execution_steps
        .saturating_add(elapsed);
}

pub(super) fn rebind_ready_generation(
    task_pool: &mut TaskPool,
    old_generation: u64,
    new_generation: u64,
) -> usize {
    let mut rebound = 0;
    for record in task_pool.tasks.values_mut() {
        if record.status == TaskStatus::Ready && record.task.registry_generation == old_generation {
            record.task.registry_generation = new_generation;
            rebound += 1;
        }
    }
    rebound
}

pub(super) fn mark_terminal_record(
    record: &mut super::TaskRecord,
    status: TaskStatus,
    failure: Option<RuntimeError>,
) {
    record.status = status;
    release_record_lease(record);
    clear_record_owner(record);
    record.failure = failure;
}

pub(super) fn release_record_lease(record: &mut super::TaskRecord) {
    record.lease = None;
    record.task.lease_id = None;
    record.claimed_by = None;
}

pub(super) fn clear_record_owner(record: &mut super::TaskRecord) {
    record.owner_runner = None;
}

pub(super) fn is_terminal_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed
            | TaskStatus::Failed
            | TaskStatus::Cancelled
            | TaskStatus::Expired
            | TaskStatus::DeadLetter
    )
}

pub(super) fn validate_record_lease(
    record: &TaskRecord,
    lease: &TaskLease,
    current_step: u64,
    action: &str,
) -> crate::RuntimeResult<()> {
    let active = record.lease.as_ref();
    let expired = task_lease_expired(lease, current_step);
    let matches_active = record.status == TaskStatus::Running
        && record.claimed_by.as_deref() == Some(lease.runner_id.as_str())
        && active.is_some_and(|active| active == lease);
    if matches_active && !expired {
        return Ok(());
    }
    let mut error = crate::runtime_error(
        ERR_TASK_CLAIM_CONFLICT,
        "runtime.task_pool",
        format!("task.{action}.{}", lease.task_id),
    );
    error.evidence.insert(
        "lease_id".into(),
        ScalarValue::String(lease.lease_id.clone()),
    );
    error.evidence.insert(
        "executor_id".into(),
        ScalarValue::String(lease.executor_id.clone()),
    );
    error
        .evidence
        .insert("current_step".into(), ScalarValue::Int(current_step as i64));
    if let Some(active) = active {
        error.evidence.insert(
            "active_lease_id".into(),
            ScalarValue::String(active.lease_id.clone()),
        );
        error.evidence.insert(
            "active_executor_id".into(),
            ScalarValue::String(active.executor_id.clone()),
        );
    }
    if expired {
        error
            .evidence
            .insert("reason".into(), ScalarValue::String("lease_expired".into()));
    }
    Err(RuntimeFailure::new(error))
}

fn task_lease_expired(lease: &TaskLease, current_step: u64) -> bool {
    lease
        .expires_at_step
        .is_some_and(|expires_at| current_step >= expires_at)
}
