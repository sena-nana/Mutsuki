use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    ERR_TASK_CLAIM_CONFLICT, ERR_TASK_DUPLICATE, ERR_TASK_NOT_FOUND, ExecutorId, RunnerDescriptor,
    RunnerId, RunnerPurity, RuntimeError, ScalarValue, SurfaceOccupancy, Task, TaskAwait, TaskId,
    TaskLease, TaskStatus, WakeCondition,
};

use crate::DispatchBudget;
use crate::{RuntimeFailure, RuntimeResult};

pub const TASK_LEASE_TTL_STEPS: u64 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct TaskRecord {
    pub task: Task,
    pub status: TaskStatus,
    pub claimed_by: Option<String>,
    pub owner_runner: Option<RunnerId>,
    pub lease: Option<TaskLease>,
    pub failure: Option<RuntimeError>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunnerLoad {
    pub running_count: usize,
    pub waiting_count: usize,
    pub queued_count: usize,
    pub pending_weight: usize,
}

#[derive(Clone, Debug, Default)]
pub struct TaskPool {
    tasks: HashMap<TaskId, TaskRecord>,
    waits_by_child: HashMap<TaskId, Vec<TaskAwait>>,
    waits_by_parent: HashMap<TaskId, Vec<TaskAwait>>,
    next_sequence: u64,
}

impl TaskPool {
    pub fn enqueue(&mut self, mut task: Task) -> RuntimeResult<TaskId> {
        let task_id = task.task_id.clone();
        if self.tasks.contains_key(&task_id) {
            return Err(crate::runtime_failure(
                ERR_TASK_DUPLICATE,
                "runtime.task_pool",
                format!("task.enqueue.{task_id}"),
            ));
        }
        self.next_sequence += 1;
        if task.created_sequence == 0 {
            task.created_sequence = self.next_sequence;
        }
        self.tasks.insert(
            task_id.clone(),
            TaskRecord {
                task,
                status: TaskStatus::Ready,
                claimed_by: None,
                owner_runner: None,
                lease: None,
                failure: None,
            },
        );
        Ok(task_id)
    }

    pub fn get(&self, task_id: &str) -> Option<&TaskRecord> {
        self.tasks.get(task_id)
    }

    pub fn records(&self) -> Vec<&TaskRecord> {
        let mut records: Vec<&TaskRecord> = self.tasks.values().collect();
        records.sort_by_key(|record| record.task.created_sequence);
        records
    }

    #[cfg(test)]
    pub fn get_mut_for_test(&mut self, task_id: &str) -> &mut TaskRecord {
        self.tasks
            .get_mut(task_id)
            .expect("test task record must exist")
    }

    pub fn ready_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|record| record.status == TaskStatus::Ready)
            .count()
    }

    pub fn running_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|record| record.status == TaskStatus::Running)
            .count()
    }

    pub fn waiting_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|record| record.status == TaskStatus::Waiting)
            .count()
    }

    pub fn running_records(&self) -> Vec<&TaskRecord> {
        let mut records: Vec<&TaskRecord> = self
            .tasks
            .values()
            .filter(|record| record.status == TaskStatus::Running)
            .collect();
        records.sort_by_key(|record| record.task.created_sequence);
        records
    }

    pub fn running_records_for_runner(&self, runner_id: &str) -> Vec<&TaskRecord> {
        self.running_records()
            .into_iter()
            .filter(|record| record.claimed_by.as_deref() == Some(runner_id))
            .collect()
    }

    pub fn waiting_records_for_runner(&self, runner_id: &str) -> Vec<&TaskRecord> {
        let mut records: Vec<&TaskRecord> = self
            .tasks
            .values()
            .filter(|record| {
                record.status == TaskStatus::Waiting
                    && record.owner_runner.as_deref() == Some(runner_id)
            })
            .collect();
        records.sort_by_key(|record| record.task.created_sequence);
        records
    }

    pub fn runner_load(
        &self,
        runner: &RunnerDescriptor,
        step: u64,
        registry_generation: u64,
    ) -> RunnerLoad {
        let running_count = self.running_records_for_runner(&runner.runner_id).len();
        let waiting_count = self.waiting_records_for_runner(&runner.runner_id).len();
        let queued_count = self
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
            .count();
        RunnerLoad {
            running_count,
            waiting_count,
            queued_count,
            pending_weight: running_count + waiting_count + queued_count,
        }
    }

    pub fn claim_ready(
        &mut self,
        runner: &RunnerDescriptor,
        step: u64,
        registry_generation: u64,
        limit: usize,
    ) -> Vec<Task> {
        self.claim_ready_for_executor(runner, "executor:inline", step, registry_generation, limit)
            .into_iter()
            .map(|(_, task)| task)
            .collect()
    }

    pub fn claim_ready_for_executor(
        &mut self,
        runner: &RunnerDescriptor,
        executor_id: impl Into<ExecutorId>,
        step: u64,
        registry_generation: u64,
        limit: usize,
    ) -> Vec<(TaskLease, Task)> {
        self.claim_ready_for_executor_with_expiry(
            runner,
            executor_id,
            step,
            registry_generation,
            limit,
            Some(step + TASK_LEASE_TTL_STEPS),
        )
    }

    pub fn claim_ready_for_executor_with_expiry(
        &mut self,
        runner: &RunnerDescriptor,
        executor_id: impl Into<ExecutorId>,
        step: u64,
        registry_generation: u64,
        limit: usize,
        expires_at_step: Option<u64>,
    ) -> Vec<(TaskLease, Task)> {
        self.claim_ready_for_executor_with_budget(
            runner,
            executor_id,
            step,
            registry_generation,
            limit,
            None,
            expires_at_step,
        )
    }

    pub fn claim_ready_for_executor_with_budget(
        &mut self,
        runner: &RunnerDescriptor,
        executor_id: impl Into<ExecutorId>,
        step: u64,
        registry_generation: u64,
        limit: usize,
        budget: Option<&DispatchBudget>,
        expires_at_step: Option<u64>,
    ) -> Vec<(TaskLease, Task)> {
        let executor_id = executor_id.into();
        let mut candidates: Vec<Task> = self
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
            if let Some(record) = self.tasks.get_mut(&task.task_id) {
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

    pub fn complete(&mut self, lease: &TaskLease, current_step: u64) -> RuntimeResult<()> {
        let record = self.leased_record_mut(lease, current_step, "complete")?;
        mark_terminal_record(record, TaskStatus::Completed, None);
        Ok(())
    }

    pub fn fail(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
        failure: RuntimeError,
    ) -> RuntimeResult<()> {
        let record = self.leased_record_mut(lease, current_step, "fail")?;
        mark_terminal_record(record, TaskStatus::Failed, Some(failure));
        Ok(())
    }

    pub fn wait(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
        ready_at_step: Option<u64>,
    ) -> RuntimeResult<()> {
        let record = self.leased_record_mut(lease, current_step, "wait")?;
        record.status = TaskStatus::Waiting;
        record.task.ready_at_step = ready_at_step;
        release_record_lease(record);
        Ok(())
    }

    pub(crate) fn defer_leased(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
    ) -> RuntimeResult<()> {
        let record = self.leased_record_mut(lease, current_step, "defer")?;
        record.status = TaskStatus::Ready;
        release_record_lease(record);
        clear_record_owner(record);
        Ok(())
    }

    pub fn wait_on_task(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
        task_await: TaskAwait,
    ) -> RuntimeResult<()> {
        if task_await.parent_task_id != lease.task_id {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.await.parent.{}", lease.task_id),
            ));
        }
        {
            let parent_record = self.record(&lease.task_id)?;
            validate_record_lease(parent_record, lease, current_step, "wait")?;
            let child_record = self.record(&task_await.child.task_id)?;
            validate_task_await_child(parent_record, child_record, &task_await)?;
        }
        let ready_at_step = ready_step_for_wait(&task_await);
        let record = self.leased_record_mut(lease, current_step, "wait")?;
        record.status = TaskStatus::Waiting;
        record.task.ready_at_step = ready_at_step;
        record.task.continuation_ref = Some(task_await.continuation.continuation.ref_id.clone());
        release_record_lease(record);
        self.waits_by_child
            .entry(task_await.child.task_id.clone())
            .or_default()
            .push(task_await.clone());
        self.waits_by_parent
            .entry(lease.task_id.clone())
            .or_default()
            .push(task_await);
        Ok(())
    }

    pub fn block(&mut self, lease: &TaskLease, current_step: u64) -> RuntimeResult<()> {
        let record = self.leased_record_mut(lease, current_step, "block")?;
        record.status = TaskStatus::Blocked;
        release_record_lease(record);
        Ok(())
    }

    pub fn wake(&mut self, task_id: &str) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if !matches!(record.status, TaskStatus::Waiting | TaskStatus::Blocked) {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.wake.{task_id}"),
            ));
        }
        record.status = TaskStatus::Ready;
        release_record_lease(record);
        self.remove_waits_for_parent(task_id);
        Ok(())
    }

    pub fn wake_due_tasks(&mut self, current_step: u64) -> Vec<(TaskId, u64)> {
        let due_tasks: Vec<_> = self
            .tasks
            .values()
            .filter_map(|record| {
                let ready_at_step = record.task.ready_at_step?;
                if matches!(record.status, TaskStatus::Waiting | TaskStatus::Blocked)
                    && ready_at_step <= current_step
                {
                    Some((record.task.task_id.clone(), ready_at_step))
                } else {
                    None
                }
            })
            .collect();
        for (task_id, _) in &due_tasks {
            if let Some(record) = self.tasks.get_mut(task_id) {
                record.status = TaskStatus::Ready;
                release_record_lease(record);
            }
            self.remove_waits_for_parent(task_id);
        }
        due_tasks
    }

    pub fn reject_ready(&mut self, task_id: &str, failure: RuntimeError) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if record.status != TaskStatus::Ready {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.reject.{task_id}"),
            ));
        }
        record.status = TaskStatus::Failed;
        record.failure = Some(failure);
        Ok(())
    }

    pub fn cancel_running_invocation(&mut self, runner_id: &str, invocation_id: &str) -> usize {
        let mut cancelled = 0;
        for record in self.tasks.values_mut() {
            if record.status != TaskStatus::Running
                || record.claimed_by.as_deref() != Some(runner_id)
            {
                continue;
            }
            if record
                .lease
                .as_ref()
                .is_some_and(|lease| lease.lease_id == invocation_id)
            {
                record.status = TaskStatus::Ready;
                release_record_lease(record);
                cancelled = 1;
                break;
            }
        }
        cancelled
    }

    pub fn cancel_task(&mut self, lease: &TaskLease, current_step: u64) -> RuntimeResult<()> {
        let record = self.leased_record_mut(lease, current_step, "cancel")?;
        mark_terminal_record(record, TaskStatus::Cancelled, None);
        self.remove_waits_for_parent(&lease.task_id);
        Ok(())
    }

    pub fn cancel_by_core(&mut self, task_id: &str) -> RuntimeResult<()> {
        self.terminal_by_core(task_id, TaskStatus::Cancelled, None, "cancel")
    }

    pub fn expire_by_core(&mut self, task_id: &str, failure: RuntimeError) -> RuntimeResult<()> {
        self.terminal_by_core(task_id, TaskStatus::Expired, Some(failure), "expire")
    }

    pub fn dead_letter_by_core(
        &mut self,
        task_id: &str,
        failure: RuntimeError,
    ) -> RuntimeResult<()> {
        self.terminal_by_core(
            task_id,
            TaskStatus::DeadLetter,
            Some(failure),
            "dead_letter",
        )
    }

    fn terminal_by_core(
        &mut self,
        task_id: &str,
        status: TaskStatus,
        failure: Option<RuntimeError>,
        action: &str,
    ) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if is_terminal_status(&record.status) {
            return Err(crate::runtime_failure(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.{action}.{task_id}"),
            ));
        }
        mark_terminal_record(record, status, failure);
        self.remove_waits_for_parent(task_id);
        Ok(())
    }

    pub fn ensure_active_lease(
        &self,
        task_id: &str,
        lease: &TaskLease,
        current_step: u64,
        action: &str,
    ) -> RuntimeResult<()> {
        validate_record_lease(self.record(task_id)?, lease, current_step, action)
    }

    pub fn reclaim_expired_task_leases(&mut self, current_step: u64) -> Vec<TaskLease> {
        let mut reclaimed = Vec::new();
        for record in self.tasks.values_mut() {
            if record.status != TaskStatus::Running {
                continue;
            }
            let expired = record
                .lease
                .as_ref()
                .and_then(|lease| lease.expires_at_step)
                .is_some_and(|expires_at| current_step >= expires_at);
            if expired {
                if let Some(lease) = record.lease.clone() {
                    reclaimed.push(lease);
                }
                record.status = TaskStatus::Ready;
                release_record_lease(record);
            }
        }
        reclaimed
    }

    pub fn reclaim_expired_leases(&mut self, current_step: u64) -> usize {
        self.reclaim_expired_task_leases(current_step).len()
    }

    pub fn awaits_for_parent(&self, task_id: &str) -> Vec<TaskAwait> {
        self.waits_by_parent
            .get(task_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn take_waits_for_child(&mut self, child_task_id: &str) -> Vec<TaskAwait> {
        let waits = self
            .waits_by_child
            .remove(child_task_id)
            .unwrap_or_default();
        for task_await in &waits {
            let remove_parent = if let Some(parent_waits) =
                self.waits_by_parent.get_mut(&task_await.parent_task_id)
            {
                parent_waits.retain(|item| item.child.task_id != child_task_id);
                parent_waits.is_empty()
            } else {
                false
            };
            if remove_parent {
                self.waits_by_parent.remove(&task_await.parent_task_id);
            }
        }
        waits
    }

    fn remove_waits_for_parent(&mut self, parent_task_id: &str) {
        let waits = self
            .waits_by_parent
            .remove(parent_task_id)
            .unwrap_or_default();
        for task_await in waits {
            let remove_child =
                if let Some(child_waits) = self.waits_by_child.get_mut(&task_await.child.task_id) {
                    child_waits.retain(|item| item.parent_task_id != parent_task_id);
                    child_waits.is_empty()
                } else {
                    false
                };
            if remove_child {
                self.waits_by_child.remove(&task_await.child.task_id);
            }
        }
    }

    pub fn rebind_ready_generation(&mut self, old_generation: u64, new_generation: u64) -> usize {
        let mut rebound = 0;
        for record in self.tasks.values_mut() {
            if record.status == TaskStatus::Ready
                && record.task.registry_generation == old_generation
            {
                record.task.registry_generation = new_generation;
                rebound += 1;
            }
        }
        rebound
    }

    pub fn surface_occupancy(&self) -> Vec<SurfaceOccupancy> {
        let mut occupancy: HashMap<String, SurfaceOccupancy> = HashMap::new();
        for record in self.tasks.values() {
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

    fn record_mut(&mut self, task_id: &str) -> RuntimeResult<&mut TaskRecord> {
        self.tasks.get_mut(task_id).ok_or_else(|| {
            crate::runtime_failure(
                ERR_TASK_NOT_FOUND,
                "runtime.task_pool",
                format!("task.{task_id}"),
            )
        })
    }

    fn record(&self, task_id: &str) -> RuntimeResult<&TaskRecord> {
        self.tasks.get(task_id).ok_or_else(|| {
            crate::runtime_failure(
                ERR_TASK_NOT_FOUND,
                "runtime.task_pool",
                format!("task.{task_id}"),
            )
        })
    }

    fn leased_record_mut(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
        action: &str,
    ) -> RuntimeResult<&mut TaskRecord> {
        let record = self.record_mut(&lease.task_id)?;
        validate_record_lease(record, lease, current_step, action)?;
        Ok(record)
    }
}

fn release_record_lease(record: &mut TaskRecord) {
    record.lease = None;
    record.task.lease_id = None;
    record.claimed_by = None;
}

fn clear_record_owner(record: &mut TaskRecord) {
    record.owner_runner = None;
}

fn mark_terminal_record(
    record: &mut TaskRecord,
    status: TaskStatus,
    failure: Option<RuntimeError>,
) {
    record.status = status;
    release_record_lease(record);
    clear_record_owner(record);
    record.failure = failure;
}

fn is_terminal_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed
            | TaskStatus::Failed
            | TaskStatus::Cancelled
            | TaskStatus::Expired
            | TaskStatus::DeadLetter
    )
}

fn validate_task_await_child(
    parent_record: &TaskRecord,
    child_record: &TaskRecord,
    task_await: &TaskAwait,
) -> RuntimeResult<()> {
    if is_terminal_status(&child_record.status) {
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

fn validate_record_lease(
    record: &TaskRecord,
    lease: &TaskLease,
    current_step: u64,
    action: &str,
) -> RuntimeResult<()> {
    let active = record.lease.as_ref();
    let expired = lease
        .expires_at_step
        .is_some_and(|expires_at| current_step >= expires_at);
    let matches_active = record.status == TaskStatus::Running
        && record.claimed_by.as_deref() == Some(lease.runner_id.as_str())
        && active.is_some_and(|active| active == lease);
    if matches_active && !expired {
        return Ok(());
    }
    let mut error = crate::runtime_error(
        ERR_TASK_CLAIM_CONFLICT,
        "runtime.task_pool",
        format!("task.{action}.{}", lease.task_id),
    );
    error.evidence.insert(
        "lease_id".into(),
        ScalarValue::String(lease.lease_id.clone()),
    );
    error.evidence.insert(
        "executor_id".into(),
        ScalarValue::String(lease.executor_id.clone()),
    );
    error
        .evidence
        .insert("current_step".into(), ScalarValue::Int(current_step as i64));
    if let Some(active) = active {
        error.evidence.insert(
            "active_lease_id".into(),
            ScalarValue::String(active.lease_id.clone()),
        );
        error.evidence.insert(
            "active_executor_id".into(),
            ScalarValue::String(active.executor_id.clone()),
        );
    }
    if expired {
        error
            .evidence
            .insert("reason".into(), ScalarValue::String("lease_expired".into()));
    }
    Err(RuntimeFailure::new(error))
}

fn ready_step_for_wait(task_await: &TaskAwait) -> Option<u64> {
    match &task_await.continuation.wake {
        Some(WakeCondition::Timer { ready_at_step })
        | Some(WakeCondition::RetryAfter { ready_at_step }) => Some(*ready_at_step),
        _ => None,
    }
}

fn surface_ids_for_record(record: &TaskRecord) -> Vec<String> {
    surface_ids_for_task(&record.task)
}

pub fn surface_ids_for_task(task: &Task) -> Vec<String> {
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

fn runner_accepts_record(
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
