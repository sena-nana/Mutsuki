use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    ERR_TASK_CLAIM_CONFLICT, ERR_TASK_NOT_FOUND, ExecutorId, RunnerDescriptor, RunnerPurity,
    RuntimeError, SurfaceOccupancy, Task, TaskAwait, TaskId, TaskLease, TaskStatus, WakeCondition,
};

use crate::{RuntimeFailure, RuntimeResult};

#[derive(Clone, Debug, PartialEq)]
pub struct TaskRecord {
    pub task: Task,
    pub status: TaskStatus,
    pub claimed_by: Option<String>,
    pub lease: Option<TaskLease>,
    pub failure: Option<RuntimeError>,
}

#[derive(Clone, Debug, Default)]
pub struct TaskPool {
    tasks: HashMap<TaskId, TaskRecord>,
    waits_by_child: HashMap<TaskId, Vec<TaskAwait>>,
    waits_by_parent: HashMap<TaskId, Vec<TaskAwait>>,
    next_sequence: u64,
}

impl TaskPool {
    pub fn enqueue(&mut self, mut task: Task) -> TaskId {
        self.next_sequence += 1;
        if task.created_sequence == 0 {
            task.created_sequence = self.next_sequence;
        }
        let task_id = task.task_id.clone();
        self.tasks.insert(
            task_id.clone(),
            TaskRecord {
                task,
                status: TaskStatus::Ready,
                claimed_by: None,
                lease: None,
                failure: None,
            },
        );
        task_id
    }

    pub fn get(&self, task_id: &str) -> Option<&TaskRecord> {
        self.tasks.get(task_id)
    }

    pub fn records(&self) -> Vec<&TaskRecord> {
        let mut records: Vec<&TaskRecord> = self.tasks.values().collect();
        records.sort_by(|a, b| a.task.created_sequence.cmp(&b.task.created_sequence));
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

    pub fn running_records(&self) -> Vec<&TaskRecord> {
        let mut records: Vec<&TaskRecord> = self
            .tasks
            .values()
            .filter(|record| record.status == TaskStatus::Running)
            .collect();
        records.sort_by(|a, b| a.task.created_sequence.cmp(&b.task.created_sequence));
        records
    }

    pub fn running_records_for_runner(&self, runner_id: &str) -> Vec<&TaskRecord> {
        self.running_records()
            .into_iter()
            .filter(|record| record.claimed_by.as_deref() == Some(runner_id))
            .collect()
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
                    && runner_accepts(runner, &record.task, registry_generation)
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
        candidates.truncate(limit);
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
                    expires_at_step: None,
                };
                record.status = TaskStatus::Running;
                record.claimed_by = Some(runner.runner_id.clone());
                record.lease = Some(lease.clone());
                record.task.lease_id = Some(lease.lease_id.clone());
                task.lease_id = Some(lease.lease_id.clone());
                leased.push((lease, task));
            }
        }
        leased
    }

    pub fn complete(&mut self, task_id: &str, runner_id: &str) -> RuntimeResult<()> {
        let record = self.claimed_record_mut(task_id, runner_id, "complete")?;
        record.status = TaskStatus::Completed;
        record.lease = None;
        Ok(())
    }

    pub fn fail(
        &mut self,
        task_id: &str,
        runner_id: &str,
        failure: RuntimeError,
    ) -> RuntimeResult<()> {
        let record = self.claimed_record_mut(task_id, runner_id, "fail")?;
        record.status = TaskStatus::Failed;
        record.lease = None;
        record.failure = Some(failure);
        Ok(())
    }

    pub fn wait(
        &mut self,
        task_id: &str,
        runner_id: &str,
        ready_at_step: Option<u64>,
    ) -> RuntimeResult<()> {
        let record = self.claimed_record_mut(task_id, runner_id, "wait")?;
        record.status = TaskStatus::Waiting;
        record.task.ready_at_step = ready_at_step;
        record.lease = None;
        Ok(())
    }

    pub fn wait_on_task(
        &mut self,
        task_id: &str,
        runner_id: &str,
        task_await: TaskAwait,
    ) -> RuntimeResult<()> {
        if task_await.parent_task_id != task_id {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.await.parent.{task_id}"),
            )));
        }
        let ready_at_step = ready_step_for_wait(&task_await);
        let record = self.claimed_record_mut(task_id, runner_id, "wait")?;
        record.status = TaskStatus::Waiting;
        record.task.ready_at_step = ready_at_step;
        record.task.continuation_ref = Some(task_await.continuation.continuation.ref_id.clone());
        record.lease = None;
        record.claimed_by = None;
        self.waits_by_child
            .entry(task_await.child.task_id.clone())
            .or_default()
            .push(task_await.clone());
        self.waits_by_parent
            .entry(task_id.to_string())
            .or_default()
            .push(task_await);
        Ok(())
    }

    pub fn block(&mut self, task_id: &str, runner_id: &str) -> RuntimeResult<()> {
        let record = self.claimed_record_mut(task_id, runner_id, "block")?;
        record.status = TaskStatus::Blocked;
        record.lease = None;
        Ok(())
    }

    pub fn wake(&mut self, task_id: &str) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if !matches!(record.status, TaskStatus::Waiting | TaskStatus::Blocked) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.wake.{task_id}"),
            )));
        }
        record.status = TaskStatus::Ready;
        record.claimed_by = None;
        record.lease = None;
        self.remove_waits_for_parent(task_id);
        Ok(())
    }

    pub fn reject_ready(&mut self, task_id: &str, failure: RuntimeError) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if record.status != TaskStatus::Ready {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.reject.{task_id}"),
            )));
        }
        record.status = TaskStatus::Failed;
        record.failure = Some(failure);
        Ok(())
    }

    pub fn cancel_running_invocation(&mut self, runner_id: &str, invocation_id: &str) -> usize {
        let mut cancelled = 0;
        if let Some(record) = self.tasks.get_mut(invocation_id)
            && record.status == TaskStatus::Running
            && record.claimed_by.as_deref() == Some(runner_id)
        {
            record.status = TaskStatus::Ready;
            record.claimed_by = None;
            record.lease = None;
            cancelled = 1;
        }
        cancelled
    }

    pub fn cancel_task(&mut self, task_id: &str, runner_id: &str) -> RuntimeResult<()> {
        let record = self.claimed_record_mut(task_id, runner_id, "cancel")?;
        record.status = TaskStatus::Cancelled;
        record.lease = None;
        self.remove_waits_for_parent(task_id);
        Ok(())
    }

    pub fn cancel_by_core(&mut self, task_id: &str) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if matches!(
            record.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.cancel.{task_id}"),
            )));
        }
        record.status = TaskStatus::Cancelled;
        record.claimed_by = None;
        record.lease = None;
        self.remove_waits_for_parent(task_id);
        Ok(())
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
                    TaskStatus::Running => {
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
            RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_NOT_FOUND,
                "runtime.task_pool",
                format!("task.{task_id}"),
            ))
        })
    }

    fn claimed_record_mut(
        &mut self,
        task_id: &str,
        runner_id: &str,
        action: &str,
    ) -> RuntimeResult<&mut TaskRecord> {
        let record = self.record_mut(task_id)?;
        if record.claimed_by.as_deref() != Some(runner_id) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.{action}.{task_id}"),
            )));
        }
        Ok(record)
    }
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
