use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    DomainEvent, ERR_TASK_UNSUPPORTED, RunnerDescriptor, RuntimeEventKind, ScalarValue, StateDelta,
    StateRollback, Task, TaskId, TaskLease,
};

use crate::{RuntimeFailure, RuntimeResult};

use super::CoreRuntime;

impl CoreRuntime {
    pub(super) fn process_kernel_tasks(
        &mut self,
        _runner: &RunnerDescriptor,
        tasks: Vec<(TaskLease, std::sync::Arc<Task>)>,
    ) -> RuntimeResult<usize> {
        let mut completed = 0;
        for (lease, task) in tasks {
            self.tasks
                .ensure_active_lease(&task.task_id, &lease, self.current_step, "kernel")?;
            match self.dispatch_kernel_task(&task) {
                Ok(()) => {
                    self.tasks.complete(&lease, self.current_step)?;
                    completed += 1;
                }
                Err(failure) => {
                    completed += self.fail_kernel_task(&lease, &task.task_id, failure)?;
                }
            }
        }
        Ok(completed)
    }

    fn dispatch_kernel_task(&mut self, task: &Task) -> RuntimeResult<()> {
        match task.protocol_id.as_str() {
            "core.commit" => self.commit_state_delta(task),
            "core.event.append" => self.append_domain_event(task),
            "core.state.rollback" => self.rollback_state(task),
            _ => Err(crate::runtime_failure(
                ERR_TASK_UNSUPPORTED,
                "runtime.committer",
                format!("core.task.{}", task.protocol_id),
            )),
        }
    }

    fn commit_state_delta(&mut self, task: &Task) -> RuntimeResult<()> {
        let delta: StateDelta = serde_json::from_value(task.payload.to_value()).map_err(|err| {
            crate::runtime_failure(
                "state.delta_decode_failed",
                "runtime.committer",
                err.to_string(),
            )
        })?;
        self.states.apply(&delta, self.current_step)?;
        let version = self
            .states
            .get(&delta.target_ref)
            .map(|(version, _)| *version)
            .unwrap_or(0);
        self.events.record(
            RuntimeEventKind::State,
            "state.commit",
            Some(delta.target_ref.to_string()),
            BTreeMap::from([
                ("version".into(), ScalarValue::Int(version as i64)),
                (
                    "expected_version".into(),
                    ScalarValue::Int(delta.expected_version as i64),
                ),
            ]),
            None,
        );
        Ok(())
    }

    fn append_domain_event(&mut self, task: &Task) -> RuntimeResult<()> {
        let event: DomainEvent =
            serde_json::from_value(task.payload.to_value()).map_err(|err| {
                crate::runtime_failure("event.decode_failed", "runtime.event_log", err.to_string())
            })?;
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "domain_event_id".into(),
            ScalarValue::String(event.event_id.clone()),
        );
        attributes.insert(
            "payload".into(),
            ScalarValue::String(event.payload.to_string()),
        );
        self.events.record(
            RuntimeEventKind::Task,
            event.kind,
            task.correlation_id.clone().or(Some(event.event_id)),
            attributes,
            None,
        );
        Ok(())
    }

    fn rollback_state(&mut self, task: &Task) -> RuntimeResult<()> {
        let request: StateRollback =
            serde_json::from_value(task.payload.to_value()).map_err(|err| {
                crate::runtime_failure(
                    "state.rollback_decode_failed",
                    "runtime.committer",
                    err.to_string(),
                )
            })?;
        let version = self.states.rollback(&request, self.current_step)?;
        self.events.record(
            RuntimeEventKind::State,
            "state.rollback",
            Some(request.target_ref.to_string()),
            BTreeMap::from([
                (
                    "to_version".into(),
                    ScalarValue::Int(request.to_version as i64),
                ),
                ("version".into(), ScalarValue::Int(version as i64)),
            ]),
            None,
        );
        Ok(())
    }

    fn fail_kernel_task(
        &mut self,
        lease: &TaskLease,
        task_id: &TaskId,
        failure: RuntimeFailure,
    ) -> RuntimeResult<usize> {
        let error = failure.error().clone();
        self.tasks.fail(lease, self.current_step, error.clone())?;
        self.record_task_terminal_event(task_id, "task.failed", Some(error));
        self.wake_tasks_waiting_on(task_id)?;
        Ok(1)
    }
}
