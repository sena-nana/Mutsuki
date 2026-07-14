use mutsuki_runtime_contracts::{ERR_TASK_CLAIM_CONFLICT, TaskAwait, TaskLease, WakeCondition};

use super::{TaskPool, transitions};

pub(super) fn wait_on_task(
    task_pool: &mut TaskPool,
    lease: &TaskLease,
    current_step: u64,
    task_await: TaskAwait,
) -> crate::RuntimeResult<()> {
    if task_await.parent_task_id != lease.task_id {
        return Err(crate::runtime_failure(
            ERR_TASK_CLAIM_CONFLICT,
            "runtime.task_pool",
            format!("task.await.parent.{}", lease.task_id),
        ));
    }
    {
        let parent_record = task_pool.record(&lease.task_id)?;
        transitions::validate_record_lease(parent_record, lease, current_step, "wait")?;
        let child_record = task_pool.record(&task_await.child.task_id)?;
        validate_task_await_child(parent_record, child_record, &task_await)?;
    }
    let ready_at_step = ready_step_for_wait(&task_await);
    {
        let record = task_pool.leased_record_mut(lease, current_step, "wait")?;
        record.status = mutsuki_runtime_contracts::TaskStatus::Waiting;
        record.task.ready_at_step = ready_at_step;
        record.task.continuation_ref = Some(task_await.continuation.continuation.ref_id.clone());
        transitions::release_record_lease(record);
    }
    task_pool.statistics.record_status_transition(
        Some(&mutsuki_runtime_contracts::TaskStatus::Running),
        Some(&mutsuki_runtime_contracts::TaskStatus::Waiting),
    );
    transitions::record_attempt_finished(task_pool, lease, current_step);
    task_pool
        .waits_by_child
        .entry(task_await.child.task_id.clone())
        .or_default()
        .push(task_await.clone());
    task_pool
        .waits_by_parent
        .entry(lease.task_id.clone())
        .or_default()
        .push(task_await);
    Ok(())
}

pub(super) fn awaits_for_parent(task_pool: &TaskPool, task_id: &str) -> Vec<TaskAwait> {
    task_pool
        .waits_by_parent
        .get(task_id)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn take_waits_for_child(
    task_pool: &mut TaskPool,
    child_task_id: &str,
) -> Vec<TaskAwait> {
    let waits = task_pool
        .waits_by_child
        .remove(child_task_id)
        .unwrap_or_default();
    for task_await in &waits {
        let remove_parent = if let Some(parent_waits) = task_pool
            .waits_by_parent
            .get_mut(&task_await.parent_task_id)
        {
            parent_waits.retain(|item| item.child.task_id != child_task_id);
            parent_waits.is_empty()
        } else {
            false
        };
        if remove_parent {
            task_pool.waits_by_parent.remove(&task_await.parent_task_id);
        }
    }
    waits
}

pub(super) fn remove_waits_for_parent(task_pool: &mut TaskPool, parent_task_id: &str) {
    let waits = task_pool
        .waits_by_parent
        .remove(parent_task_id)
        .unwrap_or_default();
    for task_await in waits {
        let remove_child = if let Some(child_waits) =
            task_pool.waits_by_child.get_mut(&task_await.child.task_id)
        {
            child_waits.retain(|item| item.parent_task_id != parent_task_id);
            child_waits.is_empty()
        } else {
            false
        };
        if remove_child {
            task_pool.waits_by_child.remove(&task_await.child.task_id);
        }
    }
}

fn validate_task_await_child(
    parent_record: &super::TaskRecord,
    child_record: &super::TaskRecord,
    task_await: &TaskAwait,
) -> crate::RuntimeResult<()> {
    if transitions::is_terminal_status(&child_record.status) {
        return Err(crate::runtime_failure(
            ERR_TASK_CLAIM_CONFLICT,
            "runtime.task_pool",
            format!("task.await.child_terminal.{}", task_await.child.task_id),
        ));
    }
    let child = &task_await.child;
    let child_matches_handle = child.protocol_id == child_record.task.protocol_id
        && child.target_binding_id == child_record.task.target_binding_id
        && child.trace_id == child_record.task.trace_id
        && child.correlation_id == child_record.task.correlation_id;
    let child_inherits_parent_context = child_record.task.trace_id == parent_record.task.trace_id
        && child_record.task.correlation_id == parent_record.task.correlation_id;
    if child_matches_handle && child_inherits_parent_context {
        return Ok(());
    }
    Err(crate::runtime_failure(
        ERR_TASK_CLAIM_CONFLICT,
        "runtime.task_pool",
        format!("task.await.child_descriptor.{}", task_await.parent_task_id),
    ))
}

fn ready_step_for_wait(task_await: &TaskAwait) -> Option<u64> {
    match &task_await.continuation.wake {
        Some(WakeCondition::Timer { ready_at_step })
        | Some(WakeCondition::RetryAfter { ready_at_step }) => Some(*ready_at_step),
        _ => None,
    }
}
