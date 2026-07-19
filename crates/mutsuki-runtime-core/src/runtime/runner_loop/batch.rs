use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, OrderingRequirement, ResourceAccessMode, ResourceReadView,
    ResourceWriteLock, RunnerDescriptor, ScalarValue, Task, TaskLease, VersionExpectation,
    WorkBatch, WorkResourcePlan, WorkSet,
};

pub(super) fn dispatch_batch_attrs(
    descriptor: &RunnerDescriptor,
    batch: &WorkBatch,
    task_leases: &[TaskLease],
    executor_id: &str,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::from([
        (
            "executor_id".into(),
            ScalarValue::String(executor_id.into()),
        ),
        (
            "task_count".into(),
            ScalarValue::Int(batch.entries.len() as i64),
        ),
        (
            "entry_count".into(),
            ScalarValue::Int(batch.entries.len() as i64),
        ),
        (
            "batch_id".into(),
            ScalarValue::String(batch.batch_id.clone()),
        ),
        ("tick_id".into(), ScalarValue::String(batch.tick_id.clone())),
        (
            "payload_layout".into(),
            ScalarValue::String(batch.payload.layout().as_str().into()),
        ),
        (
            "resource_conflict_count".into(),
            ScalarValue::Int(batch.resource_plan.conflict_entries.len() as i64),
        ),
        (
            "parallel_group_count".into(),
            ScalarValue::Int(batch.resource_plan.parallel_groups.len() as i64),
        ),
        (
            "serial_group_count".into(),
            ScalarValue::Int(batch.resource_plan.serial_groups.len() as i64),
        ),
        (
            "effective_concurrency".into(),
            ScalarValue::Int(batch.resource_plan.parallelism_limit as i64),
        ),
        (
            "runner_mode".into(),
            ScalarValue::String(format!("{:?}", descriptor.batch.mode)),
        ),
    ]);
    if let Some(entry) = batch.entries.first() {
        attrs.insert(
            "entry_id".into(),
            ScalarValue::String(entry.entry_id.clone()),
        );
        attrs.insert("task_id".into(), ScalarValue::String(entry.task_id.clone()));
        attrs.insert(
            "lane".into(),
            ScalarValue::String(format!("{:?}", entry.lane)),
        );
    }
    if let Ok(task) = batch.payload_task(0)
        && let Some(correlation_id) = &task.correlation_id
    {
        attrs.insert(
            "correlation_id".into(),
            ScalarValue::String(correlation_id.clone()),
        );
    }
    attrs.insert(
        "task_lease_ids".into(),
        ScalarValue::String(
            task_leases
                .iter()
                .map(|lease| lease.lease_id.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    attrs
}

pub(super) fn build_work_batch(
    current_step: u64,
    batch_id: &str,
    descriptor: &RunnerDescriptor,
    leased_tasks: Vec<(TaskLease, Arc<Task>)>,
) -> WorkBatch {
    let tick_id = format!("tick-{current_step}");
    let mut resource_requirements = Vec::new();
    let mut requirement_entry_indices = Vec::new();
    let mut entries = Vec::with_capacity(leased_tasks.len());
    for (payload_index, (_lease, task)) in leased_tasks.iter().enumerate() {
        let resource_start = resource_requirements.len();
        resource_requirements.extend(task.resource_requirements.clone());
        requirement_entry_indices.extend(std::iter::repeat_n(
            payload_index,
            task.resource_requirements.len(),
        ));
        let resource_requirement_indices =
            (resource_start..resource_requirements.len()).collect::<Vec<_>>();
        entries.push(BatchEntry {
            entry_id: task.task_id.clone(),
            task_id: task.task_id.clone(),
            trace_id: task.trace_id.clone(),
            parent_id: None,
            payload_index,
            resource_requirement_indices,
            cancel_index: Some(payload_index),
            deadline_tick: None,
            priority: task.priority,
            lane: task.dispatch_lane.clone(),
            ordering: task.ordering.clone(),
        });
    }
    let work_set = WorkSet {
        tick_id: tick_id.clone(),
        batch_key: descriptor.runner_id.clone(),
        entries,
        resource_requirements,
    };
    let resource_plan = build_work_resource_plan(&work_set, &requirement_entry_indices);
    let task_leases = leased_tasks
        .iter()
        .map(|(lease, _task)| lease.clone())
        .collect();
    WorkBatch {
        batch_id: batch_id.into(),
        tick_id,
        batch_key: work_set.batch_key,
        entries: work_set.entries,
        payload: BatchPayload::from_local_tasks(
            leased_tasks
                .into_iter()
                .map(|(_lease, task)| task)
                .collect(),
        ),
        resource_plan,
        task_leases,
    }
}

pub(super) fn split_leased_tasks_by_resource_conflict(
    leased_tasks: Vec<(TaskLease, Arc<Task>)>,
) -> Vec<Vec<(TaskLease, Arc<Task>)>> {
    let mut groups: Vec<Vec<(TaskLease, Arc<Task>)>> = Vec::new();
    let mut current_group: Vec<(TaskLease, Arc<Task>)> = Vec::new();
    let mut current_write_refs = HashSet::new();
    for leased_task in leased_tasks {
        let write_refs = write_requirement_refs(&leased_task.1);
        if !current_group.is_empty()
            && write_refs
                .iter()
                .any(|ref_id| current_write_refs.contains(ref_id))
        {
            groups.push(current_group);
            current_group = Vec::new();
            current_write_refs.clear();
        }
        for ref_id in write_refs {
            current_write_refs.insert(ref_id);
        }
        current_group.push(leased_task);
    }
    if !current_group.is_empty() {
        groups.push(current_group);
    }
    groups
}

fn write_requirement_refs(task: &Task) -> Vec<String> {
    task.resource_requirements
        .iter()
        .filter(|requirement| {
            matches!(
                requirement.mode,
                ResourceAccessMode::Write | ResourceAccessMode::ExclusiveWrite
            )
        })
        .map(|requirement| requirement.ref_id.clone())
        .collect()
}

fn build_work_resource_plan(
    work_set: &WorkSet,
    requirement_entry_indices: &[usize],
) -> WorkResourcePlan {
    let mut plan = WorkResourcePlan::empty();
    let mut read_views: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut write_locks: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, requirement) in work_set.resource_requirements.iter().enumerate() {
        match requirement.mode {
            ResourceAccessMode::Read => {
                read_views
                    .entry(requirement.ref_id.clone())
                    .or_default()
                    .push(index);
            }
            ResourceAccessMode::Write | ResourceAccessMode::ExclusiveWrite => {
                write_locks
                    .entry(requirement.ref_id.clone())
                    .or_default()
                    .push(index);
            }
        }
        if let Some(expected_version) = requirement.expected_version {
            plan.version_checks.push(VersionExpectation {
                ref_id: requirement.ref_id.clone(),
                expected_version,
            });
        }
    }
    plan.read_views = read_views
        .into_iter()
        .map(|(ref_id, requirement_indices)| ResourceReadView {
            ref_id,
            requirement_indices,
        })
        .collect();
    let mut conflict_entry_indices = HashSet::new();
    plan.write_locks = write_locks
        .into_iter()
        .map(|(ref_id, requirement_indices)| {
            if requirement_indices.len() > 1 {
                for requirement_index in &requirement_indices {
                    if let Some(entry_index) = requirement_entry_indices.get(*requirement_index) {
                        conflict_entry_indices.insert(*entry_index);
                    }
                }
            }
            ResourceWriteLock {
                ref_id,
                requirement_indices,
            }
        })
        .collect();
    let mut parallel_group = Vec::new();
    for (entry_index, entry) in work_set.entries.iter().enumerate() {
        if conflict_entry_indices.contains(&entry_index) {
            plan.conflict_entries.push(entry.entry_id.clone());
            continue;
        }
        match entry.ordering {
            OrderingRequirement::None => parallel_group.push(entry.entry_id.clone()),
            _ => plan.serial_groups.push(vec![entry.entry_id.clone()]),
        }
    }
    if !parallel_group.is_empty() {
        plan.parallel_groups.push(parallel_group);
    }
    plan.parallelism_limit = if plan.serial_groups.is_empty() && plan.conflict_entries.is_empty() {
        work_set.entries.len().max(1)
    } else {
        1
    };
    plan
}
