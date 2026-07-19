use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use crate::DispatchBudget;
use mutsuki_runtime_contracts::{
    ExecutorId, RunnerDescriptor, Task, TaskId, TaskLease, TaskStatus,
};

use super::{TaskPool, TaskRecord};

#[allow(clippy::too_many_arguments)]
pub(super) fn claim_ready_for_executor_with_budget(
    task_pool: &mut TaskPool,
    runner: &RunnerDescriptor,
    executor_id: impl Into<ExecutorId>,
    step: u64,
    registry_generation: u64,
    limit: usize,
    budget: Option<&DispatchBudget>,
    expires_at_step: Option<u64>,
) -> Vec<(TaskLease, Arc<Task>)> {
    let executor_id = executor_id.into();
    let candidates =
        select_candidate_ids(task_pool, runner, step, registry_generation, limit, budget);
    let mut leased = Vec::with_capacity(candidates.len());
    let mut queue_steps = 0u64;
    let mut attempts_started = 0u64;
    for task_id in candidates {
        task_pool.remove_record_indexes(&task_id);
        let (lease, task) = {
            let record = task_pool
                .tasks
                .get_mut(&task_id)
                .expect("ready index referenced a missing task record");
            debug_assert_eq!(record.status, TaskStatus::Ready);
            record.attempt_generation = record.attempt_generation.saturating_add(1);
            queue_steps = queue_steps.saturating_add(step.saturating_sub(record.ready_since_step));
            attempts_started = attempts_started.saturating_add(1);
            let lease = TaskLease {
                lease_id: format!(
                    "task-lease-{step}-{}-{}",
                    record.task.task_id, record.attempt_generation
                ),
                task_id: record.task.task_id.clone(),
                attempt_generation: record.attempt_generation,
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
            Arc::make_mut(&mut record.task).lease_id = Some(lease.lease_id.clone());
            (lease, record.task.clone())
        };
        task_pool.insert_record_indexes(&task_id);
        task_pool
            .statistics
            .record_status_transition(Some(&TaskStatus::Ready), Some(&TaskStatus::Running));
        leased.push((lease, task));
    }
    task_pool.statistics.attempts_started = task_pool
        .statistics
        .attempts_started
        .saturating_add(attempts_started);
    task_pool.statistics.cumulative_queue_steps = task_pool
        .statistics
        .cumulative_queue_steps
        .saturating_add(queue_steps);
    leased
}

pub(super) fn queued_count(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
) -> usize {
    task_pool.ready_dispatch_count(runner, step, registry_generation)
}

fn runner_accepts_indexed_task(
    _runner: &RunnerDescriptor,
    task: &Task,
    registry_generation: u64,
) -> bool {
    if registry_generation != 0
        && task.registry_generation != 0
        && task.registry_generation != registry_generation
    {
        return false;
    }
    true
}

fn select_candidate_ids(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
    limit: usize,
    budget: Option<&DispatchBudget>,
) -> Vec<TaskId> {
    if limit == 0
        || budget.is_some_and(|budget| {
            budget.max_batches == 0 || budget.max_entries == 0 || budget.max_bytes == 0
        })
    {
        return Vec::new();
    }
    let max_entries = budget.map_or(limit, |budget| limit.min(budget.max_entries));
    let mut lane_counts = HashMap::new();
    let mut selected_bytes = 0usize;
    let mut selected = Vec::with_capacity(max_entries);
    visit_candidate_records(task_pool, runner, step, registry_generation, |record| {
        if selected.len() >= max_entries {
            return false;
        }
        let payload_wire_bytes = budget
            .map(|_| task_pool.payload_wire_bytes(&record.task.task_id))
            .unwrap_or_default();
        if let Some(budget) = budget {
            if selected_bytes.saturating_add(payload_wire_bytes) > budget.max_bytes {
                return true;
            }
            if let Some(lane_budget) = budget.lane_budget.get(&record.task.dispatch_lane) {
                let used = lane_counts
                    .get(&record.task.dispatch_lane)
                    .copied()
                    .unwrap_or_default();
                if used >= lane_budget.max_entries {
                    return true;
                }
            }
        }
        *lane_counts
            .entry(record.task.dispatch_lane.clone())
            .or_insert(0) += 1;
        selected_bytes = selected_bytes.saturating_add(payload_wire_bytes);
        selected.push(record.task.task_id.clone());
        selected.len() < max_entries
    });
    selected
}

fn visit_candidate_records(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
    mut visit: impl FnMut(&TaskRecord) -> bool,
) -> usize {
    let queues = task_pool.ready_dispatch_queues(runner);
    let mut iterators = queues.iter().map(|queue| queue.iter()).collect::<Vec<_>>();
    let mut heap = BinaryHeap::new();
    for (index, iterator) in iterators.iter_mut().enumerate() {
        if let Some(key) = iterator.next()
            && key.is_due(step)
        {
            heap.push(Reverse((key, index)));
        }
    }
    let mut visited = 0;
    while let Some(Reverse((key, index))) = heap.pop() {
        if let Some(record) = task_pool.tasks.get(key.task_id())
            && runner_accepts_indexed_task(runner, &record.task, registry_generation)
        {
            visited += 1;
            if !visit(record) {
                break;
            }
        }
        if let Some(next) = iterators[index].next()
            && next.is_due(step)
        {
            heap.push(Reverse((next, index)));
        }
    }
    visited
}
