use std::collections::HashMap;

use mutsuki_runtime_contracts::{RunnerDescriptor, SurfaceOccupancy, Task, TaskStatus};

use super::claiming;
use super::{RunnerLoad, TaskPool, TaskRecord};

pub(super) fn runner_load(
    task_pool: &TaskPool,
    runner: &RunnerDescriptor,
    step: u64,
    registry_generation: u64,
) -> RunnerLoad {
    let running_count = task_pool
        .running_records_for_runner(&runner.runner_id)
        .len();
    let waiting_count = task_pool
        .waiting_records_for_runner(&runner.runner_id)
        .len();
    let queued_count = task_pool
        .tasks
        .values()
        .filter(|record| {
            record.status == TaskStatus::Ready
                && record
                    .task
                    .ready_at_step
                    .is_none_or(|ready_at| ready_at <= step)
                && claiming::runner_accepts_record(runner, record, registry_generation)
        })
        .count();
    RunnerLoad {
        running_count,
        waiting_count,
        queued_count,
        pending_weight: running_count + waiting_count + queued_count,
    }
}

pub(super) fn surface_occupancy(task_pool: &TaskPool) -> Vec<SurfaceOccupancy> {
    let mut occupancy: HashMap<String, SurfaceOccupancy> = HashMap::new();
    for record in task_pool.tasks.values() {
        for surface_id in surface_ids_for_record(record) {
            let entry = occupancy
                .entry(surface_id)
                .or_insert_with_key(|surface_id| zero_occupancy(surface_id));
            match record.status {
                TaskStatus::Ready => {
                    entry.ready_tasks += 1;
                    if record.task.protocol_id.starts_with("effect.") {
                        entry.effect_inflight += 1;
                    }
                }
                TaskStatus::Running | TaskStatus::Waiting => {
                    entry.running_invocations += 1;
                    if record.task.protocol_id.starts_with("effect.") {
                        entry.effect_inflight += 1;
                    }
                }
                _ => {}
            }
        }
    }
    let mut items: Vec<_> = occupancy.into_values().collect();
    items.sort_by(|a, b| a.surface_id.cmp(&b.surface_id));
    items
}

fn surface_ids_for_record(record: &TaskRecord) -> Vec<String> {
    surface_ids_for_task(&record.task)
}

pub(super) fn surface_ids_for_task(task: &Task) -> Vec<String> {
    let mut surface_ids = task.required_surfaces.clone();
    surface_ids.push(format!("task_protocol:{}", task.protocol_id));
    if task.protocol_id.starts_with("effect.") {
        surface_ids.push(format!("effect:{}", task.protocol_id));
    }
    if let Some(runner_hint) = &task.runner_hint {
        surface_ids.push(format!("runner:{runner_hint}"));
    }
    surface_ids.sort();
    surface_ids.dedup();
    surface_ids
}

fn zero_occupancy(surface_id: &str) -> SurfaceOccupancy {
    SurfaceOccupancy {
        surface_id: surface_id.into(),
        ready_tasks: 0,
        running_invocations: 0,
        resource_refs: 0,
        state_refs: 0,
        active_leases: 0,
        open_streams: 0,
        subscriptions: 0,
        timers: 0,
        effect_inflight: 0,
    }
}
