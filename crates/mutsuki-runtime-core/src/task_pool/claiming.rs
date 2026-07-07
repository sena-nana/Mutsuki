use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    ExecutorId, RunnerDescriptor, RunnerPurity, Task, TaskLease, TaskStatus,
};
use serde_json;

use crate::DispatchBudget;

use super::{TaskPool, TaskRecord};

pub(super) fn claim_ready_for_executor_with_budget(
    task_pool: &mut TaskPool,
    runner: &RunnerDescriptor,
    executor_id: impl Into<ExecutorId>,
    step: u64,
    registry_generation: u64,
    limit: usize,
    budget: Option<&DispatchBudget>,
    expires_at_step: Option<u64>,
) -> Vec<(TaskLease, Task)> {
    let executor_id = executor_id.into();
    let mut candidates: Vec<Task> = task_pool
        .tasks
        .values()
        .filter(|record| {
            record.status == TaskStatus::Ready
                && record
                    .task
                    .ready_at_step
                    .is_none_or(|ready_at| ready_at <= step)
                && runner_accepts_record(runner, record, registry_generation)
        })
        .map(|record| record.task.clone())
        .collect();
    candidates.sort_by(|a, b| {
        a.ready_at_step
            .unwrap_or(0)
            .cmp(&b.ready_at_step.unwrap_or(0))
            .then_with(|| b.priority.cmp(&a.priority))
            .then_with(|| a.created_sequence.cmp(&b.created_sequence))
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
    candidates = select_candidates_for_budget(candidates, limit, budget);
    let mut leased = Vec::new();
    for mut task in candidates {
        if let Some(record) = task_pool.tasks.get_mut(&task.task_id) {
            let lease = TaskLease {
                lease_id: format!("task-lease-{}-{}", step, task.task_id),
                task_id: task.task_id.clone(),
                runner_id: runner.runner_id.clone(),
                executor_id: executor_id.clone(),
                registry_generation,
                acquired_at_step: step,
                expires_at_step,
            };
            record.status = TaskStatus::Running;
            record.claimed_by = Some(runner.runner_id.clone());
            record.owner_runner = Some(runner.runner_id.clone());
            record.lease = Some(lease.clone());
            record.task.lease_id = Some(lease.lease_id.clone());
            task.lease_id = Some(lease.lease_id.clone());
            leased.push((lease, task));
        }
    }
    leased
}

pub(super) fn runner_accepts_record(
    runner: &RunnerDescriptor,
    record: &TaskRecord,
    registry_generation: u64,
) -> bool {
    if let Some(owner_runner) = &record.owner_runner
        && owner_runner != &runner.runner_id
    {
        return false;
    }
    runner_accepts(runner, &record.task, registry_generation)
}

fn runner_accepts(runner: &RunnerDescriptor, task: &Task, registry_generation: u64) -> bool {
    if registry_generation != 0
        && task.registry_generation != 0
        && task.registry_generation != registry_generation
    {
        return false;
    }
    if let Some(hint) = &task.runner_hint
        && hint != &runner.runner_id
    {
        return false;
    }
    if runner.purity == RunnerPurity::Pure
        && (task.protocol_id.starts_with("effect.") || task.protocol_id.starts_with("core."))
    {
        return false;
    }
    if runner.purity == RunnerPurity::Effectful && !task.protocol_id.starts_with("effect.") {
        return false;
    }
    if runner.purity == RunnerPurity::Committer && !task.protocol_id.starts_with("core.") {
        return false;
    }
    if task.protocol_id.starts_with("effect.") && runner.purity != RunnerPurity::Effectful {
        return false;
    }
    if task.protocol_id.starts_with("core.") && runner.purity != RunnerPurity::Committer {
        return false;
    }
    runner
        .accepted_protocol_ids
        .iter()
        .any(|protocol_id| protocol_id == &task.protocol_id)
}

fn select_candidates_for_budget(
    candidates: Vec<Task>,
    limit: usize,
    budget: Option<&DispatchBudget>,
) -> Vec<Task> {
    let Some(budget) = budget else {
        return candidates.into_iter().take(limit).collect();
    };
    if budget.max_batches == 0 || budget.max_entries == 0 || budget.max_bytes == 0 || limit == 0 {
        return Vec::new();
    }
    let max_entries = limit.min(budget.max_entries);
    let mut lane_counts = HashMap::new();
    let mut selected_bytes = 0usize;
    let mut selected = Vec::new();
    for task in candidates {
        if selected.len() >= max_entries {
            break;
        }
        let task_bytes = serde_json::to_vec(&task.payload)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if selected_bytes.saturating_add(task_bytes) > budget.max_bytes {
            continue;
        }
        if let Some(lane_budget) = budget.lane_budget.get(&task.dispatch_lane) {
            let used = lane_counts
                .get(&task.dispatch_lane)
                .copied()
                .unwrap_or_default();
            if used >= lane_budget.max_entries {
                continue;
            }
        }
        *lane_counts.entry(task.dispatch_lane.clone()).or_insert(0) += 1;
        selected_bytes = selected_bytes.saturating_add(task_bytes);
        selected.push(task);
    }
    selected
}
