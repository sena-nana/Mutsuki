use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use mutsuki_runtime_contracts::{RunnerDescriptor, TaskId, TaskStatus};

use super::{TaskPool, TaskRecord};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ReadyDispatchKey {
    protocol_id: String,
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RecordKey {
    created_sequence: u64,
    task_id: TaskId,
}

impl RecordKey {
    pub(super) fn task_id(&self) -> &str {
        &self.task_id
    }
}

#[derive(Clone, Debug)]
struct IndexSnapshot {
    task_id: TaskId,
    protocol_id: String,
    runner_hint: Option<String>,
    owner_runner: Option<String>,
    claimed_by: Option<String>,
    status: TaskStatus,
    ready_key: ReadyKey,
    record_key: RecordKey,
    ready_at_step: Option<u64>,
    has_expectations: bool,
    lease_expires_at_step: Option<u64>,
}

impl IndexSnapshot {
    fn from_record(record: &TaskRecord) -> Self {
        let task_id = record.task.task_id.clone();
        Self {
            protocol_id: record.task.protocol_id.clone(),
            runner_hint: record.task.runner_hint.clone(),
            owner_runner: record.owner_runner.clone(),
            claimed_by: record.claimed_by.clone(),
            status: record.status.clone(),
            ready_key: ReadyKey {
                ready_at_step: record.task.ready_at_step.unwrap_or(0),
                priority: Reverse(record.task.priority),
                created_sequence: record.task.created_sequence,
                task_id: task_id.clone(),
            },
            record_key: RecordKey {
                created_sequence: record.task.created_sequence,
                task_id: task_id.clone(),
            },
            ready_at_step: record.task.ready_at_step,
            has_expectations: !record.task.expected_versions.is_empty(),
            lease_expires_at_step: record
                .lease
                .as_ref()
                .and_then(|lease| lease.expires_at_step),
            task_id,
        }
    }
}

impl TaskPool {
    pub(super) fn insert_record_indexes(&mut self, task_id: &str) {
        let snapshot = self.tasks.get(task_id).map(IndexSnapshot::from_record);
        if let Some(snapshot) = snapshot {
            self.insert_snapshot(&snapshot);
        }
    }

    pub(super) fn remove_record_indexes(&mut self, task_id: &str) {
        let snapshot = self.tasks.get(task_id).map(IndexSnapshot::from_record);
        if let Some(snapshot) = snapshot {
            self.remove_snapshot(&snapshot);
        }
    }

    pub(super) fn rebuild_indexes(&mut self) {
        self.ready_by_protocol.clear();
        self.ready_by_runner_hint.clear();
        self.ready_by_owner_runner.clear();
        self.ready_by_dispatch.clear();
        self.ready_with_expectations.clear();
        self.wake_by_step.clear();
        self.running_by_runner.clear();
        self.waiting_by_runner.clear();
        self.leases_by_expiry.clear();
        let snapshots = self
            .tasks
            .values()
            .map(IndexSnapshot::from_record)
            .collect::<Vec<_>>();
        for snapshot in &snapshots {
            self.insert_snapshot(snapshot);
        }
    }

    pub(super) fn ready_dispatch_queues(
        &self,
        runner: &RunnerDescriptor,
    ) -> Vec<&BTreeSet<ReadyKey>> {
        let mut queues = Vec::new();
        for (index, protocol_id) in runner.accepted_protocol_ids.iter().enumerate() {
            if runner.accepted_protocol_ids[..index].contains(protocol_id)
                || !self.ready_by_protocol.contains_key(protocol_id)
            {
                continue;
            }
            for runner_hint in [None, Some(runner.runner_id.clone())] {
                for owner_runner in [None, Some(runner.runner_id.clone())] {
                    let key = ReadyDispatchKey {
                        protocol_id: protocol_id.clone(),
                        runner_hint: runner_hint.clone(),
                        owner_runner,
                    };
                    if let Some(queue) = self.ready_by_dispatch.get(&key) {
                        queues.push(queue);
                    }
                }
            }
        }
        queues
    }

    pub(super) fn running_record_keys(&self, runner_id: &str) -> Option<&BTreeSet<RecordKey>> {
        self.running_by_runner.get(runner_id)
    }

    pub(super) fn waiting_record_keys(&self, runner_id: &str) -> Option<&BTreeSet<RecordKey>> {
        self.waiting_by_runner.get(runner_id)
    }

    pub(super) fn running_count_for_runner(&self, runner_id: &str) -> usize {
        self.running_by_runner
            .get(runner_id)
            .map_or(0, BTreeSet::len)
    }

    pub(super) fn waiting_count_for_runner(&self, runner_id: &str) -> usize {
        self.waiting_by_runner
            .get(runner_id)
            .map_or(0, BTreeSet::len)
    }

    pub(crate) fn stale_expectation_task_ids(&self) -> Vec<TaskId> {
        self.ready_with_expectations
            .iter()
            .map(|key| key.task_id.clone())
            .collect()
    }

    pub(super) fn take_due_wake_tasks(&mut self, current_step: u64) -> Vec<(TaskId, u64)> {
        take_due_buckets(&mut self.wake_by_step, current_step)
    }

    pub(super) fn take_expired_lease_tasks(&mut self, current_step: u64) -> Vec<TaskId> {
        take_due_buckets(&mut self.leases_by_expiry, current_step)
            .into_iter()
            .map(|(task_id, _)| task_id)
            .collect()
    }

    fn insert_snapshot(&mut self, snapshot: &IndexSnapshot) {
        match snapshot.status {
            TaskStatus::Ready => {
                insert_map_set(
                    &mut self.ready_by_protocol,
                    snapshot.protocol_id.clone(),
                    snapshot.ready_key.clone(),
                );
                insert_map_set(
                    &mut self.ready_by_dispatch,
                    ReadyDispatchKey {
                        protocol_id: snapshot.protocol_id.clone(),
                        runner_hint: snapshot.runner_hint.clone(),
                        owner_runner: snapshot.owner_runner.clone(),
                    },
                    snapshot.ready_key.clone(),
                );
                if let Some(runner_hint) = &snapshot.runner_hint {
                    insert_map_set(
                        &mut self.ready_by_runner_hint,
                        runner_hint.clone(),
                        snapshot.ready_key.clone(),
                    );
                }
                if let Some(owner_runner) = &snapshot.owner_runner {
                    insert_map_set(
                        &mut self.ready_by_owner_runner,
                        owner_runner.clone(),
                        snapshot.ready_key.clone(),
                    );
                }
                if snapshot.has_expectations {
                    self.ready_with_expectations
                        .insert(snapshot.record_key.clone());
                }
            }
            TaskStatus::Running => {
                if let Some(claimed_by) = &snapshot.claimed_by {
                    insert_map_set(
                        &mut self.running_by_runner,
                        claimed_by.clone(),
                        snapshot.record_key.clone(),
                    );
                }
                if let Some(expires_at_step) = snapshot.lease_expires_at_step {
                    self.leases_by_expiry
                        .entry(expires_at_step)
                        .or_default()
                        .insert(snapshot.task_id.clone());
                }
            }
            TaskStatus::Waiting => {
                if let Some(owner_runner) = &snapshot.owner_runner {
                    insert_map_set(
                        &mut self.waiting_by_runner,
                        owner_runner.clone(),
                        snapshot.record_key.clone(),
                    );
                }
                self.insert_wake_snapshot(snapshot);
            }
            TaskStatus::Blocked => self.insert_wake_snapshot(snapshot),
            _ => {}
        }
    }

    fn remove_snapshot(&mut self, snapshot: &IndexSnapshot) {
        match snapshot.status {
            TaskStatus::Ready => {
                remove_map_set(
                    &mut self.ready_by_protocol,
                    &snapshot.protocol_id,
                    &snapshot.ready_key,
                );
                remove_map_set(
                    &mut self.ready_by_dispatch,
                    &ReadyDispatchKey {
                        protocol_id: snapshot.protocol_id.clone(),
                        runner_hint: snapshot.runner_hint.clone(),
                        owner_runner: snapshot.owner_runner.clone(),
                    },
                    &snapshot.ready_key,
                );
                if let Some(runner_hint) = &snapshot.runner_hint {
                    remove_map_set(
                        &mut self.ready_by_runner_hint,
                        runner_hint,
                        &snapshot.ready_key,
                    );
                }
                if let Some(owner_runner) = &snapshot.owner_runner {
                    remove_map_set(
                        &mut self.ready_by_owner_runner,
                        owner_runner,
                        &snapshot.ready_key,
                    );
                }
                if snapshot.has_expectations {
                    self.ready_with_expectations.remove(&snapshot.record_key);
                }
            }
            TaskStatus::Running => {
                if let Some(claimed_by) = &snapshot.claimed_by {
                    remove_map_set(
                        &mut self.running_by_runner,
                        claimed_by,
                        &snapshot.record_key,
                    );
                }
                if let Some(expires_at_step) = snapshot.lease_expires_at_step {
                    remove_bucket_task(
                        &mut self.leases_by_expiry,
                        expires_at_step,
                        &snapshot.task_id,
                    );
                }
            }
            TaskStatus::Waiting => {
                if let Some(owner_runner) = &snapshot.owner_runner {
                    remove_map_set(
                        &mut self.waiting_by_runner,
                        owner_runner,
                        &snapshot.record_key,
                    );
                }
                self.remove_wake_snapshot(snapshot);
            }
            TaskStatus::Blocked => self.remove_wake_snapshot(snapshot),
            _ => {}
        }
    }

    fn insert_wake_snapshot(&mut self, snapshot: &IndexSnapshot) {
        if let Some(ready_at_step) = snapshot.ready_at_step {
            self.wake_by_step
                .entry(ready_at_step)
                .or_default()
                .insert(snapshot.task_id.clone());
        }
    }

    fn remove_wake_snapshot(&mut self, snapshot: &IndexSnapshot) {
        if let Some(ready_at_step) = snapshot.ready_at_step {
            remove_bucket_task(&mut self.wake_by_step, ready_at_step, &snapshot.task_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn assert_indexes_consistent(&self) {
        let mut rebuilt = self.clone();
        rebuilt.rebuild_indexes();
        assert_eq!(self.ready_by_protocol, rebuilt.ready_by_protocol);
        assert_eq!(self.ready_by_runner_hint, rebuilt.ready_by_runner_hint);
        assert_eq!(self.ready_by_owner_runner, rebuilt.ready_by_owner_runner);
        assert_eq!(self.ready_by_dispatch, rebuilt.ready_by_dispatch);
        assert_eq!(
            self.ready_with_expectations,
            rebuilt.ready_with_expectations
        );
        assert_eq!(self.wake_by_step, rebuilt.wake_by_step);
        assert_eq!(self.running_by_runner, rebuilt.running_by_runner);
        assert_eq!(self.waiting_by_runner, rebuilt.waiting_by_runner);
        assert_eq!(self.leases_by_expiry, rebuilt.leases_by_expiry);
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
    pub(crate) fn rebuild_indexes_for_test(&mut self) {
        self.rebuild_indexes();
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
        self.ready_with_expectations.len()
            + self.wake_by_step.values().map(BTreeSet::len).sum::<usize>()
            + self
                .leases_by_expiry
                .values()
                .map(BTreeSet::len)
                .sum::<usize>()
    }
}

fn insert_map_set<K, V>(map: &mut HashMap<K, BTreeSet<V>>, key: K, value: V)
where
    K: std::hash::Hash + Eq,
    V: Ord,
{
    map.entry(key).or_default().insert(value);
}

fn remove_map_set<K, V>(map: &mut HashMap<K, BTreeSet<V>>, key: &K, value: &V)
where
    K: std::hash::Hash + Eq,
    V: Ord,
{
    let remove_entry = map.get_mut(key).is_some_and(|values| {
        values.remove(value);
        values.is_empty()
    });
    if remove_entry {
        map.remove(key);
    }
}

fn remove_bucket_task(buckets: &mut BTreeMap<u64, BTreeSet<TaskId>>, step: u64, task_id: &str) {
    let remove_bucket = buckets.get_mut(&step).is_some_and(|tasks| {
        tasks.remove(task_id);
        tasks.is_empty()
    });
    if remove_bucket {
        buckets.remove(&step);
    }
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
