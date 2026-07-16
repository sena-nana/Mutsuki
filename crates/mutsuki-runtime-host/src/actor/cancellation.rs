use std::collections::BTreeMap;
use std::time::Instant;

use crate::management::ManagementExecutor;

use super::RunningBatch;

pub(super) fn request_running_cancel(
    invocation_id: &str,
    management: &ManagementExecutor,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) {
    if running_batches_by_task
        .values()
        .any(|task| task.invocation_id == invocation_id && task.cancel_requested_at.is_some())
    {
        return;
    }
    let target = running_batches_by_task
        .values()
        .find(|task| task.invocation_id == invocation_id)
        .map(|task| (task.runner_id.clone(), task.management.clone()));
    mark_cancel_requested(invocation_id, running_batches_by_task);
    let Some((runner_id, handle)) = target else {
        return;
    };
    let delivered = handle.as_ref().is_some_and(|handle| {
        management
            .cancel(runner_id.clone(), invocation_id.into(), handle.clone())
            .is_ok()
    });
    if !delivered {
        let pending = pending_cancels.entry(runner_id).or_default();
        if !pending.iter().any(|pending| pending == invocation_id) {
            pending.push(invocation_id.into());
        }
    }
}

pub(super) fn queue_management_retry(
    runner_id: String,
    invocation_id: String,
    running_batches_by_task: &BTreeMap<String, RunningBatch>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) {
    if !running_batches_by_task
        .values()
        .any(|task| task.invocation_id == invocation_id)
    {
        return;
    }
    let pending = pending_cancels.entry(runner_id).or_default();
    if !pending.contains(&invocation_id) {
        pending.push(invocation_id);
    }
}

fn mark_cancel_requested(
    invocation_id: &str,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) {
    let now = Instant::now();
    for task in running_batches_by_task.values_mut() {
        if task.invocation_id == invocation_id {
            task.cancel_requested_at = Some(now);
        }
    }
}
