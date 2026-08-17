use mutsuki_runtime_contracts::{
    ERR_TASK_CLAIM_CONFLICT, RuntimeError, ScalarValue, TaskLease, TaskStatus,
};

use crate::{RuntimeFailure, RuntimeResult};
use serde_json::Value;

use super::{PendingCancellation, TaskPool, TaskRecord};

pub(super) fn complete(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
    output: Option<Value>,
) -> RuntimeResult<()> {
    task_pool.mutate_record_indexed(&lease.task_id, move |record| {
        validate_record_lease(record, lease, current_step, "complete")?;
        record.output = output;
        mark_terminal_record(record, TaskStatus::Completed, None);
        Ok(())
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Completed));
    task_pool.pending_cancellations.remove(&lease.task_id);
    record_attempt_finished(task_pool, lease, current_step);
    task_pool.record_terminal_task(&lease.task_id);
    Ok(())
}

pub(super) fn fail(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
    failure: RuntimeError,
) -> RuntimeResult<()> {
    task_pool.mutate_record_indexed(&lease.task_id, |record| {
        validate_record_lease(record, lease, current_step, "fail")?;
        mark_terminal_record(record, TaskStatus::Failed, Some(failure));
        Ok(())
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Failed));
    task_pool.pending_cancellations.remove(&lease.task_id);
    record_attempt_finished(task_pool, lease, current_step);
    task_pool.record_terminal_task(&lease.task_id);
    Ok(())
}

pub(super) fn wait(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
    ready_at_step: Option<u64>,
) -> RuntimeResult<()> {
    task_pool.mutate_record_indexed(&lease.task_id, |record| {
        validate_record_lease(record, lease, current_step, "wait")?;
        record.status = TaskStatus::Waiting;
        std::sync::Arc::make_mut(&mut record.task).ready_at_step = ready_at_step;
        release_record_lease(record);
        Ok(())
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Waiting));
    task_pool.pending_cancellations.remove(&lease.task_id);
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn defer_leased(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<()> {
    task_pool.mutate_record_indexed(&lease.task_id, |record| {
        validate_record_lease(record, lease, current_step, "defer")?;
        record.status = TaskStatus::Ready;
        record.ready_since_step = current_step;
        release_record_lease(record);
        clear_record_owner(record);
        Ok(())
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Ready));
    task_pool.pending_cancellations.remove(&lease.task_id);
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn block(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<()> {
    task_pool.mutate_record_indexed(&lease.task_id, |record| {
        validate_record_lease(record, lease, current_step, "block")?;
        record.status = TaskStatus::Blocked;
        release_record_lease(record);
        Ok(())
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Blocked));
    task_pool.pending_cancellations.remove(&lease.task_id);
    record_attempt_finished(task_pool, lease, current_step);
    Ok(())
}

pub(super) fn wake(
    task_pool: &mut TaskPool,
    task_id: impl AsRef<str>,
    current_step: u64,
) -> RuntimeResult<()> {
    let task_id = task_id.as_ref();
    let previous_status = task_pool.mutate_record_indexed(task_id, |record| {
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
        Ok(previous_status)
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&previous_status), Some(&TaskStatus::Ready));
    crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
    Ok(())
}

pub(super) fn wake_due_tasks(
    task_pool: &mut TaskPool,
    current_step: u64,
) -> Vec<(mutsuki_runtime_contracts::TaskId, u64)> {
    let due_tasks = task_pool.take_due_wake_tasks(current_step);
    for (task_id, _) in &due_tasks {
        let previous_status = task_pool
            .mutate_record_indexed(task_id, |record| {
                let previous_status = record.status.clone();
                record.status = TaskStatus::Ready;
                record.ready_since_step = current_step;
                release_record_lease(record);
                Ok(previous_status)
            })
            .expect("wake index referenced a missing task record");
        task_pool
            .statistics
            .record_status_transition(Some(&previous_status), Some(&TaskStatus::Ready));
        crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
    }
    due_tasks
}

pub(super) fn reject_ready(
    task_pool: &mut TaskPool,
    task_id: impl AsRef<str>,
    failure: RuntimeError,
) -> RuntimeResult<()> {
    let task_id = task_id.as_ref();
    task_pool.mutate_record_indexed(task_id, |record| {
        if record.status != TaskStatus::Ready {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.reject.{task_id}"),
            ));
        }
        record.status = TaskStatus::Failed;
        record.failure = Some(failure);
        Ok(())
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Ready), Some(&TaskStatus::Failed));
    task_pool.record_terminal_task(task_id);
    Ok(())
}

pub(super) fn cancel_running_invocation(
    task_pool: &mut TaskPool,
    runner_id: impl AsRef<str>,
    invocation_id: impl AsRef<str>,
    current_step: u64,
) -> usize {
    let runner_id = runner_id.as_ref();
    let invocation_id = invocation_id.as_ref();
    let task_id = task_pool
        .running_task_ids(runner_id)
        .into_iter()
        .flatten()
        .find_map(|task_id| {
            task_pool.tasks.get(task_id).and_then(|record| {
                record
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.lease_id.as_str() == invocation_id)
                    .then(|| record.task.task_id.clone())
            })
        });
    let Some(task_id) = task_id else {
        return 0;
    };
    let lease = task_pool
        .tasks
        .get(&task_id)
        .and_then(|record| record.lease.clone());
    task_pool
        .mutate_record_indexed(&task_id, |record| {
            record.status = TaskStatus::Ready;
            record.ready_since_step = current_step;
            release_record_lease(record);
            Ok(())
        })
        .expect("running index referenced a missing task record");
    if let Some(lease) = lease {
        record_attempt_finished_value(&mut task_pool.statistics, &lease, current_step);
    }
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Ready));
    task_pool.pending_cancellations.remove(&task_id);
    1
}

pub(super) fn cancel_task(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<()> {
    task_pool.mutate_record_indexed(&lease.task_id, |record| {
        validate_record_lease(record, lease, current_step, "cancel")?;
        mark_terminal_record(record, TaskStatus::Cancelled, None);
        Ok(())
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Cancelled));
    task_pool.pending_cancellations.remove(&lease.task_id);
    record_attempt_finished(task_pool, lease, current_step);
    crate::task_pool::awaits::remove_waits_for_parent(task_pool, &lease.task_id);
    task_pool.record_terminal_task(&lease.task_id);
    Ok(())
}

pub(super) fn request_cancel_by_core(
    task_pool: &mut TaskPool,
    task_id: impl AsRef<str>,
    current_step: u64,
    failure: Option<RuntimeError>,
) -> RuntimeResult<bool> {
    let task_id = task_id.as_ref();
    if let Some(pending) = task_pool.pending_cancellations.get_mut(task_id) {
        if failure.is_some() {
            pending.failure = failure;
        }
        return Ok(false);
    }
    let status = task_pool.record(task_id)?.status.clone();
    if status == TaskStatus::Running {
        task_pool
            .pending_cancellations
            .insert(task_id.into(), PendingCancellation { failure });
        return Ok(false);
    }
    terminal_by_core(
        task_pool,
        task_id,
        TaskStatus::Cancelled,
        failure,
        "cancel",
        current_step,
    )?;
    Ok(true)
}

pub(super) fn finalize_requested_cancellation(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
) -> RuntimeResult<Option<PendingCancellation>> {
    let Some(pending) = task_pool.pending_cancellations.get(&lease.task_id).cloned() else {
        return Ok(None);
    };
    task_pool.mutate_record_indexed(&lease.task_id, |record| {
        let matches_active = record.status == TaskStatus::Running
            && record.claimed_by.as_ref() == Some(&lease.runner_id)
            && record.lease.as_ref() == Some(lease);
        if !matches_active {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.cancel_finalize.{}", lease.task_id),
            ));
        }
        mark_terminal_record(record, TaskStatus::Cancelled, pending.failure.clone());
        Ok(())
    })?;
    task_pool.pending_cancellations.remove(&lease.task_id);
    task_pool
        .statistics
        .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Cancelled));
    record_attempt_finished(task_pool, lease, current_step);
    crate::task_pool::awaits::remove_waits_for_parent(task_pool, &lease.task_id);
    task_pool.record_terminal_task(&lease.task_id);
    Ok(Some(pending))
}

pub(super) fn terminal_by_core(
    task_pool: &mut TaskPool,
    task_id: impl AsRef<str>,
    status: TaskStatus,
    failure: Option<RuntimeError>,
    action: &str,
    current_step: u64,
) -> RuntimeResult<()> {
    let task_id = task_id.as_ref();
    let (active_lease, previous_status) = task_pool.mutate_record_indexed(task_id, |record| {
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
        Ok((active_lease, previous_status))
    })?;
    task_pool
        .statistics
        .record_status_transition(Some(&previous_status), Some(&status));
    task_pool.pending_cancellations.remove(task_id);
    if let Some(lease) = active_lease {
        record_attempt_finished(task_pool, &lease, current_step);
    }
    crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
    task_pool.record_terminal_task(task_id);
    Ok(())
}

pub(super) fn ensure_active_lease(
    task_pool: &TaskPool,
    task_id: impl AsRef<str>,
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
    let task_ids = task_pool.take_expired_lease_tasks(current_step);
    let mut reclaimed = Vec::new();
    for task_id in task_ids {
        if task_pool.pending_cancellations.contains_key(&task_id) {
            continue;
        }
        let lease = task_pool
            .tasks
            .get(&task_id)
            .and_then(|record| record.lease.clone());
        if lease
            .as_ref()
            .is_some_and(|lease| task_lease_expired(lease, current_step))
        {
            task_pool
                .mutate_record_indexed(&task_id, |record| {
                    record.status = TaskStatus::Ready;
                    record.ready_since_step = current_step;
                    release_record_lease(record);
                    Ok(())
                })
                .expect("lease expiry index referenced a missing task record");
            task_pool
                .statistics
                .record_status_transition(Some(&TaskStatus::Running), Some(&TaskStatus::Ready));
            reclaimed.push(lease.expect("expired lease checked above"));
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
) -> Vec<mutsuki_runtime_contracts::TaskId> {
    let mut aborted = Vec::new();
    let mut finished_leases = Vec::new();
    let mut pending = Vec::new();
    let mut finalized = Vec::new();
    for record in task_pool.tasks.values_mut() {
        if is_terminal_status(&record.status) {
            continue;
        }
        aborted.push(record.task.task_id.clone());
        if record.status == TaskStatus::Running {
            pending.push(record.task.task_id.clone());
            continue;
        }
        if let Some(lease) = record.lease.clone() {
            finished_leases.push(lease);
        }
        finalized.push(record.task.task_id.clone());
        let previous_status = record.status.clone();
        mark_terminal_record(record, TaskStatus::Cancelled, Some(failure.clone()));
        task_pool
            .statistics
            .record_status_transition(Some(&previous_status), Some(&TaskStatus::Cancelled));
    }
    for task_id in pending {
        task_pool.pending_cancellations.insert(
            task_id,
            PendingCancellation {
                failure: Some(failure.clone()),
            },
        );
    }
    task_pool.rebuild_indexes();
    for lease in &finished_leases {
        record_attempt_finished(task_pool, lease, current_step);
    }
    aborted.sort();
    for task_id in &finalized {
        crate::task_pool::awaits::remove_waits_for_parent(task_pool, task_id);
        task_pool.record_terminal_task(task_id);
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
            std::sync::Arc::make_mut(&mut record.task).registry_generation = new_generation;
            rebound += 1;
        }
    }
    task_pool.rebuild_indexes();
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
    std::sync::Arc::make_mut(&mut record.task).lease_id = None;
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
        && record.claimed_by.as_ref() == Some(&lease.runner_id)
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
        ScalarValue::String(lease.lease_id.to_string()),
    );
    error.evidence.insert(
        "executor_id".into(),
        ScalarValue::String(lease.executor_id.to_string()),
    );
    error
        .evidence
        .insert("current_step".into(), ScalarValue::Int(current_step as i64));
    if let Some(active) = active {
        error.evidence.insert(
            "active_lease_id".into(),
            ScalarValue::String(active.lease_id.to_string()),
        );
        error.evidence.insert(
            "active_executor_id".into(),
            ScalarValue::String(active.executor_id.to_string()),
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
