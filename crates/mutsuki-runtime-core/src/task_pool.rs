use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    ERR_TASK_NOT_FOUND, ExecutorId, RunnerDescriptor, RunnerId, RuntimeError, SurfaceOccupancy,
    Task, TaskAwait, TaskId, TaskLease, TaskStatus,
};

use crate::DispatchBudget;
use crate::RuntimeResult;

mod awaits;
mod claiming;
mod occupancy;
mod transitions;

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
                mutsuki_runtime_contracts::ERR_TASK_DUPLICATE,
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
        occupancy::runner_load(self, runner, step, registry_generation)
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
        claiming::claim_ready_for_executor_with_budget(
            self,
            runner,
            executor_id,
            step,
            registry_generation,
            limit,
            budget,
            expires_at_step,
        )
    }

    pub fn complete(&mut self, lease: &TaskLease, current_step: u64) -> RuntimeResult<()> {
        transitions::complete(self, lease, current_step)
    }

    pub fn fail(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
        failure: RuntimeError,
    ) -> RuntimeResult<()> {
        transitions::fail(self, lease, current_step, failure)
    }

    pub fn wait(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
        ready_at_step: Option<u64>,
    ) -> RuntimeResult<()> {
        transitions::wait(self, lease, current_step, ready_at_step)
    }

    pub(crate) fn defer_leased(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
    ) -> RuntimeResult<()> {
        transitions::defer_leased(self, lease, current_step)
    }

    pub fn wait_on_task(
        &mut self,
        lease: &TaskLease,
        current_step: u64,
        task_await: TaskAwait,
    ) -> RuntimeResult<()> {
        awaits::wait_on_task(self, lease, current_step, task_await)
    }

    pub fn block(&mut self, lease: &TaskLease, current_step: u64) -> RuntimeResult<()> {
        transitions::block(self, lease, current_step)
    }

    pub fn wake(&mut self, task_id: &str) -> RuntimeResult<()> {
        transitions::wake(self, task_id)
    }

    pub fn wake_due_tasks(&mut self, current_step: u64) -> Vec<(TaskId, u64)> {
        transitions::wake_due_tasks(self, current_step)
    }

    pub fn reject_ready(&mut self, task_id: &str, failure: RuntimeError) -> RuntimeResult<()> {
        transitions::reject_ready(self, task_id, failure)
    }

    pub fn cancel_running_invocation(&mut self, runner_id: &str, invocation_id: &str) -> usize {
        transitions::cancel_running_invocation(self, runner_id, invocation_id)
    }

    pub fn cancel_task(&mut self, lease: &TaskLease, current_step: u64) -> RuntimeResult<()> {
        transitions::cancel_task(self, lease, current_step)
    }

    pub fn cancel_by_core(&mut self, task_id: &str) -> RuntimeResult<()> {
        transitions::terminal_by_core(self, task_id, TaskStatus::Cancelled, None, "cancel")
    }

    pub fn expire_by_core(&mut self, task_id: &str, failure: RuntimeError) -> RuntimeResult<()> {
        transitions::terminal_by_core(self, task_id, TaskStatus::Expired, Some(failure), "expire")
    }

    pub fn dead_letter_by_core(
        &mut self,
        task_id: &str,
        failure: RuntimeError,
    ) -> RuntimeResult<()> {
        transitions::terminal_by_core(
            self,
            task_id,
            TaskStatus::DeadLetter,
            Some(failure),
            "dead_letter",
        )
    }

    pub(crate) fn ensure_active_lease(
        &self,
        task_id: &str,
        lease: &TaskLease,
        current_step: u64,
        action: &str,
    ) -> RuntimeResult<()> {
        transitions::ensure_active_lease(self, task_id, lease, current_step, action)
    }

    pub(crate) fn reclaim_expired_task_leases(&mut self, current_step: u64) -> Vec<TaskLease> {
        transitions::reclaim_expired_task_leases(self, current_step)
    }

    pub(crate) fn surface_ids_for_task(&self, task: &Task) -> Vec<String> {
        occupancy::surface_ids_for_task(task)
    }

    pub fn awaits_for_parent(&self, task_id: &str) -> Vec<TaskAwait> {
        awaits::awaits_for_parent(self, task_id)
    }

    pub fn take_waits_for_child(&mut self, child_task_id: &str) -> Vec<TaskAwait> {
        awaits::take_waits_for_child(self, child_task_id)
    }

    pub fn rebind_ready_generation(&mut self, old_generation: u64, new_generation: u64) -> usize {
        transitions::rebind_ready_generation(self, old_generation, new_generation)
    }

    pub fn surface_occupancy(&self) -> Vec<SurfaceOccupancy> {
        occupancy::surface_occupancy(self)
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
        transitions::validate_record_lease(record, lease, current_step, action)?;
        Ok(record)
    }
}
