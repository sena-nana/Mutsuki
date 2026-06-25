use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    ERR_TASK_CLAIM_CONFLICT, ERR_TASK_NOT_FOUND, RunnerDescriptor, RunnerPurity, RuntimeError,
    Task, TaskId, TaskStatus,
};

use crate::{RuntimeFailure, RuntimeResult};

#[derive(Clone, Debug, PartialEq)]
pub struct TaskRecord {
    pub task: Task,
    pub status: TaskStatus,
    pub claimed_by: Option<String>,
    pub failure: Option<RuntimeError>,
}

#[derive(Clone, Debug, Default)]
pub struct TaskPool {
    tasks: HashMap<TaskId, TaskRecord>,
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
                status: TaskStatus::Pending,
                claimed_by: None,
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

    pub fn pending_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|record| record.status == TaskStatus::Pending)
            .count()
    }

    pub fn running_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|record| record.status == TaskStatus::Running)
            .count()
    }

    pub fn claim_ready(
        &mut self,
        runner: &RunnerDescriptor,
        step: u64,
        registry_generation: u64,
        limit: usize,
    ) -> Vec<Task> {
        let mut candidates: Vec<Task> = self
            .tasks
            .values()
            .filter(|record| {
                record.status == TaskStatus::Pending
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
        for task in &candidates {
            if let Some(record) = self.tasks.get_mut(&task.task_id) {
                record.status = TaskStatus::Running;
                record.claimed_by = Some(runner.runner_id.clone());
            }
        }
        candidates
    }

    pub fn complete(&mut self, task_id: &str, runner_id: &str) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if record.claimed_by.as_deref() != Some(runner_id) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.complete.{task_id}"),
            )));
        }
        record.status = TaskStatus::Completed;
        Ok(())
    }

    pub fn fail(
        &mut self,
        task_id: &str,
        runner_id: &str,
        failure: RuntimeError,
    ) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if record.claimed_by.as_deref() != Some(runner_id) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_TASK_CLAIM_CONFLICT,
                "runtime.task_pool",
                format!("task.fail.{task_id}"),
            )));
        }
        record.status = TaskStatus::Failed;
        record.failure = Some(failure);
        Ok(())
    }

    pub fn reject_pending(&mut self, task_id: &str, failure: RuntimeError) -> RuntimeResult<()> {
        let record = self.record_mut(task_id)?;
        if record.status != TaskStatus::Pending {
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

    pub fn cancel_running_for_runner(&mut self, runner_id: &str) -> usize {
        let mut cancelled = 0;
        for record in self.tasks.values_mut() {
            if record.status == TaskStatus::Running
                && record.claimed_by.as_deref() == Some(runner_id)
            {
                record.status = TaskStatus::Pending;
                record.claimed_by = None;
                cancelled += 1;
            }
        }
        cancelled
    }

    pub fn rebind_pending_generation(&mut self, old_generation: u64, new_generation: u64) -> usize {
        let mut rebound = 0;
        for record in self.tasks.values_mut() {
            if record.status == TaskStatus::Pending
                && record.task.registry_generation == old_generation
            {
                record.task.registry_generation = new_generation;
                rebound += 1;
            }
        }
        rebound
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
}

fn runner_accepts(runner: &RunnerDescriptor, task: &Task, registry_generation: u64) -> bool {
    if registry_generation != 0
        && task.registry_generation != 0
        && task.registry_generation != registry_generation
    {
        return false;
    }
    if let Some(hint) = &task.runner_hint {
        if hint != &runner.runner_id {
            return false;
        }
    }
    if runner.purity == RunnerPurity::Pure
        && (task.kind.starts_with("effect.") || task.kind.starts_with("core."))
    {
        return false;
    }
    if runner.purity == RunnerPurity::Effectful && !task.kind.starts_with("effect.") {
        return false;
    }
    if runner.purity == RunnerPurity::Committer && !task.kind.starts_with("core.") {
        return false;
    }
    if task.kind.starts_with("effect.") && runner.purity != RunnerPurity::Effectful {
        return false;
    }
    if task.kind.starts_with("core.") && runner.purity != RunnerPurity::Committer {
        return false;
    }
    runner
        .accepted_task_kinds
        .iter()
        .any(|kind| kind == &task.kind)
}
