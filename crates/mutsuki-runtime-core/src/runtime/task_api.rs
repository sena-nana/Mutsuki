use std::collections::{BTreeMap, BTreeSet};

use mutsuki_runtime_contracts::ScalarValue;
use mutsuki_runtime_contracts::{
    CancelPolicy, ERR_TASK_DEAD_LETTER, ERR_TASK_DUPLICATE, ERR_TASK_EXPIRED, RuntimeEvent,
    RuntimeEventKind, Task, TaskBatch, TaskHandle, TaskOutcome, TaskStatus,
};
use serde_json::Value;

use crate::{RuntimeResult, TaskPool};

use super::{CoreRuntime, TaskResultSnapshot};

impl CoreRuntime {
    /// Returns the task and owning runner pairs affected by a cascade cancellation.
    /// Hosts use this snapshot before terminal transitions clear runner ownership.
    pub fn task_cancellation_targets(&self, handle: &TaskHandle) -> Vec<(String, String)> {
        let mut pending = vec![handle.task_id.clone()];
        let mut visited = BTreeSet::new();
        let mut targets = Vec::new();
        while let Some(task_id) = pending.pop() {
            if !visited.insert(task_id.clone()) {
                continue;
            }
            if let Some(record) = self.tasks.get(&task_id)
                && let Some(runner_id) = &record.owner_runner
            {
                targets.push((task_id.clone(), runner_id.clone()));
            }
            pending.extend(
                self.tasks
                    .awaits_for_parent(&task_id)
                    .into_iter()
                    .filter(|task_await| task_await.cancel_policy == CancelPolicy::Cascade)
                    .map(|task_await| task_await.child.task_id),
            );
        }
        targets.sort();
        targets
    }

    pub(crate) fn enqueue_task(&mut self, mut task: Task) -> RuntimeResult<String> {
        self.ensure_not_aborted()?;
        if task.registry_generation == 0 {
            task.registry_generation = self.load_plan.registry_generation;
        }
        let deprecated_surface = self
            .tasks
            .surface_ids_for_task(&task)
            .into_iter()
            .find(|surface_id| self.is_surface_deprecated(surface_id));
        let task_id = self.tasks.enqueue_at(task, self.current_step)?;
        if let Some(surface_id) = deprecated_surface {
            let _ = self.tasks.reject_ready(
                &task_id,
                crate::runtime_error(
                    mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                    "runtime.result_router",
                    format!("surface.deprecated.{surface_id}"),
                ),
            );
        }
        self.events.record(
            RuntimeEventKind::Task,
            "task.enqueue",
            Some(task_id.clone()),
            BTreeMap::new(),
            None,
        );
        self.events.record(
            RuntimeEventKind::Task,
            "task.submitted",
            Some(task_id.clone()),
            BTreeMap::new(),
            None,
        );
        Ok(task_id)
    }

    pub fn submit_task(&mut self, task: Task) -> RuntimeResult<TaskHandle> {
        let batch_id = format!("core.submit.{}", task.task_id);
        self.submit_batch(TaskBatch::one(batch_id, task))?
            .into_iter()
            .next()
            .ok_or_else(|| {
                crate::runtime_failure(
                    mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                    "runtime.task",
                    "task.submit.empty",
                )
            })
    }

    pub fn submit_batch(&mut self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.ensure_accepting_external_tasks()?;
        let mut seen = BTreeMap::new();
        for task in &batch.tasks {
            if seen.insert(task.task_id.clone(), ()).is_some()
                || self.tasks.contains_task_id(&task.task_id)
            {
                return Err(crate::runtime_failure(
                    ERR_TASK_DUPLICATE,
                    "runtime.task",
                    format!("task.batch.{}.{}", batch.batch_id, task.task_id),
                ));
            }
        }
        let mut task_ids = Vec::with_capacity(batch.tasks.len());
        for task in batch.tasks {
            task_ids.push(self.enqueue_task(task)?);
        }
        task_ids
            .iter()
            .map(|task_id| self.task_handle_for_id(task_id))
            .collect()
    }

    pub fn submit_targeted_task(
        &mut self,
        task_id: &str,
        binding_id: &str,
        payload: Value,
    ) -> RuntimeResult<TaskHandle> {
        let binding = self
            .handler_bindings
            .all()
            .iter()
            .find(|binding| binding.binding_id == binding_id)
            .ok_or_else(|| {
                crate::runtime_failure(
                    mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
                    "runtime.handler_binding",
                    format!("handler_binding.{binding_id}"),
                )
            })?;
        let mut task = Task::new(task_id, &binding.target_protocol_id, payload);
        task.target_binding_id = Some(binding.binding_id.clone());
        task.runner_hint = binding.target_runner_hint.clone();
        self.submit_task(task)
    }

    fn task_handle_for_id(&self, task_id: &str) -> RuntimeResult<TaskHandle> {
        let record = self.tasks.get(task_id).ok_or_else(|| {
            crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_TASK_NOT_FOUND,
                "runtime.task",
                format!("task.handle.{task_id}"),
            )
        })?;
        Ok(TaskHandle {
            task_id: record.task.task_id.clone(),
            protocol_id: record.task.protocol_id.clone(),
            target_binding_id: record.task.target_binding_id.clone(),
            cancel_policy: CancelPolicy::Cascade,
            trace_id: record.task.trace_id.clone(),
            correlation_id: record.task.correlation_id.clone(),
        })
    }

    pub(crate) fn task_status(&self, task_id: &str) -> Option<TaskStatus> {
        self.tasks.get(task_id).map(|record| record.status.clone())
    }

    pub fn task_handle_status(&self, handle: &TaskHandle) -> Option<TaskStatus> {
        self.task_status(&handle.task_id)
    }

    pub(crate) fn task_result(&self, task_id: &str) -> Option<TaskResultSnapshot> {
        self.tasks.get(task_id).map(|record| TaskResultSnapshot {
            task_id: record.task.task_id.clone(),
            status: record.status.clone(),
            output: record.output.clone(),
            output_ref: record.task.output_ref.clone(),
            continuation_ref: record.task.continuation_ref.clone(),
            failure: record.failure.clone(),
        })
    }

    pub fn task_handle_result(&self, handle: &TaskHandle) -> Option<TaskResultSnapshot> {
        self.task_result(&handle.task_id)
    }

    pub(crate) fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
        let record = self.tasks.get(task_id).ok_or_else(|| {
            crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_TASK_NOT_FOUND,
                "runtime.task",
                format!("task.outcome.{task_id}"),
            )
        })?;
        Ok(match record.status {
            TaskStatus::Completed => Some(TaskOutcome::Completed {
                task_id: record.task.task_id.clone(),
                output: record.output.clone(),
                output_ref: record.task.output_ref.clone(),
            }),
            TaskStatus::Failed => Some(TaskOutcome::Failed {
                task_id: record.task.task_id.clone(),
                error: record.failure.clone().unwrap_or_else(|| {
                    crate::runtime_error(
                        "runner.failed",
                        "runtime.task",
                        format!("task.outcome.{task_id}"),
                    )
                }),
            }),
            TaskStatus::Cancelled => Some(TaskOutcome::Cancelled {
                task_id: record.task.task_id.clone(),
                reason: record.failure.as_ref().map(|error| error.route.clone()),
            }),
            TaskStatus::Expired => Some(TaskOutcome::Expired {
                task_id: record.task.task_id.clone(),
                reason: record.failure.as_ref().map(|error| error.route.clone()),
            }),
            TaskStatus::DeadLetter => Some(TaskOutcome::DeadLetter {
                task_id: record.task.task_id.clone(),
                reason: record.failure.as_ref().map(|error| error.route.clone()),
            }),
            _ => None,
        })
    }

    pub fn task_handle_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.task_outcome(&handle.task_id)
    }

    pub(crate) fn task_events(&self, task_id: &str) -> Vec<&RuntimeEvent> {
        self.events
            .iter()
            .filter(|event| event.subject_id.as_deref() == Some(task_id))
            .collect()
    }

    pub fn task_handle_events(&self, handle: &TaskHandle) -> Vec<&RuntimeEvent> {
        self.task_events(&handle.task_id)
    }

    pub fn events_after(
        &self,
        sequence: u64,
        limit: usize,
    ) -> mutsuki_runtime_contracts::ObservabilityPage<RuntimeEvent> {
        self.events.page_after(sequence, limit)
    }

    pub(crate) fn cancel_task_by_id(&mut self, task_id: &str) -> RuntimeResult<()> {
        let awaits = self.tasks.awaits_for_parent(task_id);
        if awaits
            .iter()
            .any(|task_await| task_await.cancel_policy != CancelPolicy::Cascade)
        {
            return Err(crate::runtime_failure(
                "task.cancel_policy_unsupported",
                "runtime.task",
                format!("task.cancel.{task_id}"),
            ));
        }
        self.tasks.cancel_by_core(task_id, self.current_step)?;
        self.record_task_terminal_event(task_id, "task.cancelled", None);
        for task_await in awaits {
            if matches!(
                self.task_status(&task_await.child.task_id),
                Some(
                    TaskStatus::Created
                        | TaskStatus::Ready
                        | TaskStatus::Running
                        | TaskStatus::Waiting
                        | TaskStatus::Blocked
                )
            ) {
                self.cancel_task_by_id(&task_await.child.task_id)?;
            }
        }
        self.wake_tasks_waiting_on(task_id)?;
        Ok(())
    }

    pub fn cancel_task_handle(&mut self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.cancel_task_by_id(&handle.task_id)
    }

    pub fn expire_task(&mut self, task_id: &str, reason: impl Into<String>) -> RuntimeResult<()> {
        let mut failure = crate::runtime_error(
            ERR_TASK_EXPIRED,
            "runtime.task",
            format!("task.expire.{task_id}"),
        );
        failure
            .evidence
            .insert("reason".into(), ScalarValue::String(reason.into()));
        self.tasks
            .expire_by_core(task_id, failure.clone(), self.current_step)?;
        self.record_task_terminal_event(task_id, "task.expired", Some(failure));
        self.wake_tasks_waiting_on(task_id)?;
        Ok(())
    }

    pub fn dead_letter_task(
        &mut self,
        task_id: &str,
        reason: impl Into<String>,
    ) -> RuntimeResult<()> {
        let mut failure = crate::runtime_error(
            ERR_TASK_DEAD_LETTER,
            "runtime.task",
            format!("task.dead_letter.{task_id}"),
        );
        failure
            .evidence
            .insert("reason".into(), ScalarValue::String(reason.into()));
        self.tasks
            .dead_letter_by_core(task_id, failure.clone(), self.current_step)?;
        self.record_task_terminal_event(task_id, "task.dead_lettered", Some(failure));
        self.wake_tasks_waiting_on(task_id)?;
        Ok(())
    }

    pub(crate) fn wake_task_by_id(&mut self, task_id: &str) -> RuntimeResult<()> {
        self.tasks.wake(task_id, self.current_step)
    }

    pub fn wake_task_handle(&mut self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.wake_task_by_id(&handle.task_id)
    }

    pub fn tasks(&self) -> &TaskPool {
        &self.tasks
    }
}
