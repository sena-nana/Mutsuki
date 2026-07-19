use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;

use mutsuki_runtime_contracts::{RunnerDescriptor, TaskId, TaskStatus};

use super::{TaskPool, TaskRecord};

type ReadyQueues = HashMap<String, HashMap<ReadySelector, BTreeSet<ReadyKey>>>;
type ReadyCounts = HashMap<String, HashMap<ReadySelector, BTreeMap<(u64, u64), usize>>>;

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
    ready_by_protocol: ReadyQueues,
    ready_counts_by_protocol: ReadyCounts,
    ready_by_step: BTreeMap<u64, BTreeSet<TaskId>>,
    ready_with_expectations: BTreeSet<TaskId>,
    wake_by_step: BTreeMap<u64, BTreeSet<TaskId>>,
    running_by_runner: HashMap<String, BTreeSet<TaskId>>,
    waiting_by_runner: HashMap<String, BTreeSet<TaskId>>,
    leases_by_expiry: BTreeMap<u64, BTreeSet<TaskId>>,
}

impl TaskPool {
    pub(super) fn insert_record_indexes(&mut self, task_id: &str) {
        self.update_record_indexes(task_id, true);
    }

    pub(super) fn remove_record_indexes(&mut self, task_id: &str) {
        self.update_record_indexes(task_id, false);
    }

    fn update_record_indexes(&mut self, task_id: &str, present: bool) {
        if let Some(record) = self.tasks.get(task_id) {
            self.indexes.update_record(record, present);
        }
    }

    pub(super) fn rebuild_indexes(&mut self) {
        self.indexes = TaskIndexes::default();
        for record in self.tasks.values() {
            self.indexes.update_record(record, true);
        }
    }

    pub(super) fn ready_dispatch_queues(
        &self,
        runner: &RunnerDescriptor,
    ) -> Vec<&BTreeSet<ReadyKey>> {
        let mut queues = Vec::new();
        self.with_ready_selectors(&runner.runner_id, |selectors| {
            for protocol_id in &runner.accepted_protocol_ids {
                let Some(by_selector) = self.indexes.ready_by_protocol.get(protocol_id) else {
                    continue;
                };
                for selector in selectors {
                    if let Some(queue) = by_selector.get(selector) {
                        queues.push(queue);
                    }
                }
            }
        });
        queues
    }

    pub(super) fn ready_dispatch_count(
        &self,
        runner: &RunnerDescriptor,
        current_step: u64,
        registry_generation: u64,
    ) -> usize {
        self.with_ready_selectors(&runner.runner_id, |selectors| {
            runner
                .accepted_protocol_ids
                .iter()
                .filter_map(|protocol_id| self.indexes.ready_counts_by_protocol.get(protocol_id))
                .flat_map(|by_selector| {
                    selectors
                        .iter()
                        .filter_map(|selector| by_selector.get(selector))
                })
                .flat_map(|by_step| by_step.iter())
                .filter(|((ready_at_step, task_generation), _count)| {
                    *ready_at_step <= current_step
                        && (registry_generation == 0
                            || *task_generation == 0
                            || *task_generation == registry_generation)
                })
                .map(|(_key, count)| count)
                .copied()
                .sum()
        })
    }

    fn with_ready_selectors<R>(
        &self,
        runner_id: &str,
        use_selectors: impl FnOnce(&[ReadySelector; 4]) -> R,
    ) -> R {
        let mut cache = self.ready_selector_cache.borrow_mut();
        if cache.contains_key(runner_id) {
            return use_selectors(cache.get(runner_id).expect("cached selector exists"));
        }
        if cache.len() < super::READY_SELECTOR_CACHE_CAPACITY {
            cache.insert(runner_id.into(), build_ready_selectors(runner_id));
            return use_selectors(cache.get(runner_id).expect("inserted selector exists"));
        }
        drop(cache);
        use_selectors(&build_ready_selectors(runner_id))
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

impl TaskIndexes {
    fn update_record(&mut self, record: &TaskRecord, present: bool) {
        match record.status {
            TaskStatus::Ready => {
                let selector = ReadySelector {
                    runner_hint: record.task.runner_hint.clone(),
                    owner_runner: record.owner_runner.clone(),
                };
                let ready_at_step = record.task.ready_at_step.unwrap_or(0);
                let ready_key = ReadyKey {
                    ready_at_step,
                    priority: Reverse(record.task.priority),
                    created_sequence: record.task.created_sequence,
                    task_id: record.task.task_id.clone(),
                };
                set_ready(
                    &mut self.ready_by_protocol,
                    &record.task.protocol_id,
                    &selector,
                    &ready_key,
                    present,
                );
                set_ready_count(
                    &mut self.ready_counts_by_protocol,
                    &record.task.protocol_id,
                    &selector,
                    ready_at_step,
                    record.task.registry_generation,
                    present,
                );
                if !record.task.expected_versions.is_empty() {
                    set_value(
                        &mut self.ready_with_expectations,
                        &record.task.task_id,
                        present,
                    );
                }
                if let Some(step) = record.task.ready_at_step {
                    set_bucket(&mut self.ready_by_step, step, &record.task.task_id, present);
                }
            }
            TaskStatus::Running => {
                if let Some(runner_id) = &record.claimed_by {
                    set_map_value(
                        &mut self.running_by_runner,
                        runner_id,
                        &record.task.task_id,
                        present,
                    );
                }
                if let Some(step) = record
                    .lease
                    .as_ref()
                    .and_then(|lease| lease.expires_at_step)
                {
                    set_bucket(
                        &mut self.leases_by_expiry,
                        step,
                        &record.task.task_id,
                        present,
                    );
                }
            }
            TaskStatus::Waiting => {
                if let Some(runner_id) = &record.owner_runner {
                    set_map_value(
                        &mut self.waiting_by_runner,
                        runner_id,
                        &record.task.task_id,
                        present,
                    );
                }
                self.update_wake(record, present);
            }
            TaskStatus::Blocked => self.update_wake(record, present),
            _ => {}
        }
    }

    fn update_wake(&mut self, record: &TaskRecord, present: bool) {
        if let Some(step) = record.task.ready_at_step {
            set_bucket(&mut self.wake_by_step, step, &record.task.task_id, present);
        }
    }
}

fn build_ready_selectors(runner_id: &str) -> [ReadySelector; 4] {
    let runner_id = runner_id.to_owned();
    [
        ReadySelector::default(),
        ReadySelector {
            runner_hint: Some(runner_id.clone()),
            owner_runner: None,
        },
        ReadySelector {
            runner_hint: None,
            owner_runner: Some(runner_id.clone()),
        },
        ReadySelector {
            runner_hint: Some(runner_id.clone()),
            owner_runner: Some(runner_id),
        },
    ]
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
    index: &mut ReadyQueues,
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

fn set_ready_count(
    index: &mut ReadyCounts,
    protocol_id: &str,
    selector: &ReadySelector,
    ready_at_step: u64,
    registry_generation: u64,
    present: bool,
) {
    let bucket = (ready_at_step, registry_generation);
    if present {
        *index
            .entry(protocol_id.into())
            .or_default()
            .entry(selector.clone())
            .or_default()
            .entry(bucket)
            .or_default() += 1;
        return;
    }
    let remove_protocol = index.get_mut(protocol_id).is_some_and(|by_selector| {
        let remove_selector = by_selector.get_mut(selector).is_some_and(|by_bucket| {
            let remove_bucket = by_bucket.get_mut(&bucket).is_some_and(|count| {
                *count = count.saturating_sub(1);
                *count == 0
            });
            if remove_bucket {
                by_bucket.remove(&bucket);
            }
            by_bucket.is_empty()
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
