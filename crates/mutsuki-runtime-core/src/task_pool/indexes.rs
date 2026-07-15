use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;

use mutsuki_runtime_contracts::{RunnerDescriptor, TaskId, TaskStatus};

use super::{TaskPool, TaskRecord};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct ReadySelector {
    runner_hint: Option<String>,
    owner_runner: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ReadyKey {
    ready_at_step: u64,
    priority: Reverse<i64>,
    created_sequence: u64,
    task_id: TaskId,
}

impl ReadyKey {
    pub(super) fn task_id(&self) -> &str {
        &self.task_id
    }

    pub(super) fn is_due(&self, current_step: u64) -> bool {
        self.ready_at_step <= current_step
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TaskIndexes {
    ready_by_protocol: HashMap<String, HashMap<ReadySelector, BTreeSet<ReadyKey>>>,
    ready_by_step: BTreeMap<u64, BTreeSet<TaskId>>,
    ready_with_expectations: BTreeSet<TaskId>,
    wake_by_step: BTreeMap<u64, BTreeSet<TaskId>>,
    running_by_runner: HashMap<String, BTreeSet<TaskId>>,
    waiting_by_runner: HashMap<String, BTreeSet<TaskId>>,
    leases_by_expiry: BTreeMap<u64, BTreeSet<TaskId>>,
}

#[derive(Clone, Debug)]
struct IndexSnapshot {
    task_id: TaskId,
    protocol_id: String,
    selector: ReadySelector,
    claimed_by: Option<String>,
    status: TaskStatus,
    ready_key: ReadyKey,
    ready_at_step: Option<u64>,
    has_expectations: bool,
    lease_expires_at_step: Option<u64>,
}

impl IndexSnapshot {
    fn from_record(record: &TaskRecord) -> Self {
        let task_id = record.task.task_id.clone();
        Self {
            task_id: task_id.clone(),
            protocol_id: record.task.protocol_id.clone(),
            selector: ReadySelector {
                runner_hint: record.task.runner_hint.clone(),
                owner_runner: record.owner_runner.clone(),
            },
            claimed_by: record.claimed_by.clone(),
            status: record.status.clone(),
            ready_key: ReadyKey {
                ready_at_step: record.task.ready_at_step.unwrap_or(0),
                priority: Reverse(record.task.priority),
                created_sequence: record.task.created_sequence,
                task_id: task_id.clone(),
            },
            ready_at_step: record.task.ready_at_step,
            has_expectations: !record.task.expected_versions.is_empty(),
            lease_expires_at_step: record
                .lease
                .as_ref()
                .and_then(|lease| lease.expires_at_step),
        }
    }
}

impl TaskPool {
    pub(super) fn insert_record_indexes(&mut self, task_id: &str) {
        self.update_record_indexes(task_id, true);
    }

    pub(super) fn remove_record_indexes(&mut self, task_id: &str) {
        self.update_record_indexes(task_id, false);
    }

    fn update_record_indexes(&mut self, task_id: &str, present: bool) {
        if let Some(snapshot) = self.tasks.get(task_id).map(IndexSnapshot::from_record) {
            self.update_snapshot(&snapshot, present);
        }
    }

    pub(super) fn rebuild_indexes(&mut self) {
        self.indexes = TaskIndexes::default();
        let snapshots = self
            .tasks
            .values()
            .map(IndexSnapshot::from_record)
            .collect::<Vec<_>>();
        for snapshot in &snapshots {
            self.update_snapshot(snapshot, true);
        }
    }

    pub(super) fn ready_dispatch_queues(
        &self,
        runner: &RunnerDescriptor,
    ) -> Vec<&BTreeSet<ReadyKey>> {
        let mut queues = Vec::new();
        for (index, protocol_id) in runner.accepted_protocol_ids.iter().enumerate() {
            if runner.accepted_protocol_ids[..index].contains(protocol_id) {
                continue;
            }
            let Some(by_selector) = self.indexes.ready_by_protocol.get(protocol_id) else {
                continue;
            };
            for runner_hint in [None, Some(runner.runner_id.clone())] {
                for owner_runner in [None, Some(runner.runner_id.clone())] {
                    if let Some(queue) = by_selector.get(&ReadySelector {
                        runner_hint: runner_hint.clone(),
                        owner_runner,
                    }) {
                        queues.push(queue);
                    }
                }
            }
        }
        queues
    }

    pub(super) fn running_task_ids(&self, runner_id: &str) -> Option<&BTreeSet<TaskId>> {
        self.indexes.running_by_runner.get(runner_id)
    }

    pub(super) fn waiting_task_ids(&self, runner_id: &str) -> Option<&BTreeSet<TaskId>> {
        self.indexes.waiting_by_runner.get(runner_id)
    }

    pub(super) fn running_count_for_runner(&self, runner_id: &str) -> usize {
        map_set_len(&self.indexes.running_by_runner, runner_id)
    }

    pub(super) fn waiting_count_for_runner(&self, runner_id: &str) -> usize {
        map_set_len(&self.indexes.waiting_by_runner, runner_id)
    }

    pub(crate) fn stale_expectation_task_ids(&self) -> Vec<TaskId> {
        self.indexes
            .ready_with_expectations
            .iter()
            .cloned()
            .collect()
    }

    pub(super) fn take_due_wake_tasks(&mut self, current_step: u64) -> Vec<(TaskId, u64)> {
        take_due_buckets(&mut self.indexes.wake_by_step, current_step)
    }

    pub(super) fn take_expired_lease_tasks(&mut self, current_step: u64) -> Vec<TaskId> {
        take_due_buckets(&mut self.indexes.leases_by_expiry, current_step)
            .into_iter()
            .map(|(task_id, _)| task_id)
            .collect()
    }

    /// Returns the earliest future logical step at which indexed task state can change.
    /// Already-due ready tasks are excluded because their originating event drives dispatch.
    pub fn next_required_step_after(&self, current_step: u64) -> Option<u64> {
        [
            next_bucket_after(&self.indexes.ready_by_step, current_step),
            next_bucket_after(&self.indexes.wake_by_step, current_step),
            next_bucket_after(&self.indexes.leases_by_expiry, current_step),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn update_snapshot(&mut self, snapshot: &IndexSnapshot, present: bool) {
        match snapshot.status {
            TaskStatus::Ready => {
                set_ready(
                    &mut self.indexes.ready_by_protocol,
                    &snapshot.protocol_id,
                    &snapshot.selector,
                    &snapshot.ready_key,
                    present,
                );
                if snapshot.has_expectations {
                    set_value(
                        &mut self.indexes.ready_with_expectations,
                        &snapshot.task_id,
                        present,
                    );
                }
                if let Some(step) = snapshot.ready_at_step {
                    set_bucket(
                        &mut self.indexes.ready_by_step,
                        step,
                        &snapshot.task_id,
                        present,
                    );
                }
            }
            TaskStatus::Running => {
                if let Some(runner_id) = &snapshot.claimed_by {
                    set_map_value(
                        &mut self.indexes.running_by_runner,
                        runner_id,
                        &snapshot.task_id,
                        present,
                    );
                }
                if let Some(step) = snapshot.lease_expires_at_step {
                    set_bucket(
                        &mut self.indexes.leases_by_expiry,
                        step,
                        &snapshot.task_id,
                        present,
                    );
                }
            }
            TaskStatus::Waiting => {
                if let Some(runner_id) = &snapshot.selector.owner_runner {
                    set_map_value(
                        &mut self.indexes.waiting_by_runner,
                        runner_id,
                        &snapshot.task_id,
                        present,
                    );
                }
                self.update_wake(snapshot, present);
            }
            TaskStatus::Blocked => self.update_wake(snapshot, present),
            _ => {}
        }
    }

    fn update_wake(&mut self, snapshot: &IndexSnapshot, present: bool) {
        if let Some(step) = snapshot.ready_at_step {
            set_bucket(
                &mut self.indexes.wake_by_step,
                step,
                &snapshot.task_id,
                present,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn assert_indexes_consistent(&self) {
        let mut rebuilt = self.clone();
        rebuilt.rebuild_indexes();
        assert_eq!(self.indexes, rebuilt.indexes);
        assert_eq!(self.payload_wire_bytes.len(), self.tasks.len());
        for record in self.tasks.values() {
            assert_eq!(
                self.payload_wire_bytes(&record.task.task_id),
                serde_json::to_vec(&record.task.payload)
                    .expect("task payload must remain serializable")
                    .len()
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn ready_dispatch_candidate_count_for_test(
        &self,
        runner: &RunnerDescriptor,
    ) -> usize {
        self.ready_dispatch_queues(runner)
            .into_iter()
            .map(BTreeSet::len)
            .sum()
    }

    #[cfg(test)]
    pub(crate) fn pending_tick_index_entries_for_test(&self) -> usize {
        self.indexes.ready_with_expectations.len()
            + self
                .indexes
                .wake_by_step
                .values()
                .map(BTreeSet::len)
                .sum::<usize>()
            + self
                .indexes
                .leases_by_expiry
                .values()
                .map(BTreeSet::len)
                .sum::<usize>()
    }
}

fn next_bucket_after(buckets: &BTreeMap<u64, BTreeSet<TaskId>>, current_step: u64) -> Option<u64> {
    buckets
        .range((
            std::ops::Bound::Excluded(current_step),
            std::ops::Bound::Unbounded,
        ))
        .next()
        .map(|(step, _)| *step)
}

fn set_ready(
    index: &mut HashMap<String, HashMap<ReadySelector, BTreeSet<ReadyKey>>>,
    protocol_id: &str,
    selector: &ReadySelector,
    key: &ReadyKey,
    present: bool,
) {
    if present {
        index
            .entry(protocol_id.into())
            .or_default()
            .entry(selector.clone())
            .or_default()
            .insert(key.clone());
        return;
    }
    let remove_protocol = index.get_mut(protocol_id).is_some_and(|by_selector| {
        let remove_selector = by_selector.get_mut(selector).is_some_and(|queue| {
            queue.remove(key);
            queue.is_empty()
        });
        if remove_selector {
            by_selector.remove(selector);
        }
        by_selector.is_empty()
    });
    if remove_protocol {
        index.remove(protocol_id);
    }
}

fn set_map_value<K, V>(map: &mut HashMap<K, BTreeSet<V>>, key: &K, value: &V, present: bool)
where
    K: Clone + Hash + Eq,
    V: Clone + Ord,
{
    if present {
        map.entry(key.clone()).or_default().insert(value.clone());
    } else if map.get_mut(key).is_some_and(|values| {
        values.remove(value);
        values.is_empty()
    }) {
        map.remove(key);
    }
}

fn set_value<V: Clone + Ord>(set: &mut BTreeSet<V>, value: &V, present: bool) {
    if present {
        set.insert(value.clone());
    } else {
        set.remove(value);
    }
}

fn set_bucket(
    buckets: &mut BTreeMap<u64, BTreeSet<TaskId>>,
    step: u64,
    task_id: &str,
    present: bool,
) {
    if present {
        buckets.entry(step).or_default().insert(task_id.into());
    } else if buckets.get_mut(&step).is_some_and(|tasks| {
        tasks.remove(task_id);
        tasks.is_empty()
    }) {
        buckets.remove(&step);
    }
}

fn map_set_len<V>(map: &HashMap<String, BTreeSet<V>>, key: &str) -> usize {
    map.get(key).map_or(0, BTreeSet::len)
}

fn take_due_buckets(
    buckets: &mut BTreeMap<u64, BTreeSet<TaskId>>,
    current_step: u64,
) -> Vec<(TaskId, u64)> {
    let due = if current_step == u64::MAX {
        std::mem::take(buckets)
    } else {
        let future = buckets.split_off(&(current_step + 1));
        std::mem::replace(buckets, future)
    };
    due.into_iter()
        .flat_map(|(step, task_ids)| task_ids.into_iter().map(move |task_id| (task_id, step)))
        .collect()
}
