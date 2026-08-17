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
                )
                .into(),
                task_id: record.task.task_id.clone(),
                attempt_generation: record.attempt_generation,
                runner_id: runner.runner_id.as_str().into(),
                executor_id: executor_id.clone(),
                registry_generation,
                acquired_at_step: step,
                expires_at_step,
            };
            record.status = TaskStatus::Running;
            record.claimed_by = Some(runner.runner_id.as_str().into());
            record.owner_runner = Some(runner.runner_id.as_str().into());
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

pub(super) fn queued_count_for_lane(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
    lane: &mutsuki_runtime_contracts::DispatchLane,
) -> usize {
    task_pool.ready_dispatch_count_for_lane(runner, step, registry_generation, lane)
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
    if let Some(budget) = budget
        && !budget.lane_budget.is_empty()
    {
        return select_qos_candidate_ids(
            task_pool,
            runner,
            step,
            registry_generation,
            max_entries,
            budget,
        );
    }
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

fn select_qos_candidate_ids(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
    max_entries: usize,
    budget: &DispatchBudget,
) -> Vec<TaskId> {
    let mut candidates = HashMap::new();
    for lane in mutsuki_runtime_contracts::DispatchLane::ALL {
        let lane_limit = budget
            .lane_budget
            .get(&lane)
            .map_or(max_entries, |lane_budget| lane_budget.max_entries)
            .min(max_entries);
        if lane_limit == 0 {
            continue;
        }
        let mut ids = Vec::with_capacity(lane_limit);
        visit_candidate_records_for_lane(
            task_pool,
            runner,
            step,
            registry_generation,
            &lane,
            |record| {
                ids.push(record.task.task_id.clone());
                ids.len() < lane_limit
            },
        );
        if !ids.is_empty() {
            candidates.insert(lane, ids.into_iter());
        }
    }

    let mut selected = Vec::with_capacity(max_entries);
    let mut selected_bytes = 0usize;
    while selected.len() < max_entries {
        let mut progressed = false;
        for lane in mutsuki_runtime_contracts::DispatchLane::ALL {
            let Some(iter) = candidates.get_mut(&lane) else {
                continue;
            };
            let Some(task_id) = iter.next() else {
                continue;
            };
            let payload_bytes = task_pool.payload_wire_bytes(&task_id);
            if selected_bytes.saturating_add(payload_bytes) > budget.max_bytes {
                continue;
            }
            selected_bytes = selected_bytes.saturating_add(payload_bytes);
            selected.push(task_id);
            progressed = true;
            if selected.len() >= max_entries {
                break;
            }
        }
        if !progressed {
            break;
        }
    }
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
    visit_queues(
        task_pool,
        runner,
        step,
        registry_generation,
        queues,
        &mut visit,
    )
}

fn visit_candidate_records_for_lane(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
    lane: &mutsuki_runtime_contracts::DispatchLane,
    mut visit: impl FnMut(&TaskRecord) -> bool,
) -> usize {
    let queues = task_pool.ready_dispatch_queues_for_lane(runner, lane);
    visit_queues(
        task_pool,
        runner,
        step,
        registry_generation,
        queues,
        &mut visit,
    )
}

fn visit_queues(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
    queues: Vec<&std::collections::BTreeSet<super::indexes::ReadyKey>>,
    visit: &mut impl FnMut(&TaskRecord) -> bool,
) -> usize {
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
