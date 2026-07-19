use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    DomainEvent, ERR_TASK_UNSUPPORTED, RunnerDescriptor, RuntimeEventKind, StateDelta, Task,
    TaskLease,
};

use crate::RuntimeResult;

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
            match task.protocol_id.as_str() {
                "core.commit" => {
                    let delta: StateDelta =
                        serde_json::from_value(task.payload.clone()).map_err(|err| {
                            crate::runtime_failure(
                                "state.delta_decode_failed",
                                "runtime.committer",
                                err.to_string(),
                            )
                        })?;
                    self.states.apply(&delta)?;
                    self.events.record(
                        RuntimeEventKind::State,
                        "state.commit",
                        Some(delta.target_ref),
                        BTreeMap::new(),
                        None,
                    );
                }
                "core.event.append" => {
                    let event: DomainEvent =
                        serde_json::from_value(task.payload.clone()).map_err(|err| {
                            crate::runtime_failure(
                                "event.decode_failed",
                                "runtime.event_log",
                                err.to_string(),
                            )
                        })?;
                    self.events.record(
                        RuntimeEventKind::Task,
                        event.kind,
                        Some(event.event_id),
                        BTreeMap::new(),
                        None,
                    );
                }
                _ => {
                    let failure = crate::runtime_error(
                        ERR_TASK_UNSUPPORTED,
                        "runtime.committer",
                        format!("core.task.{}", task.protocol_id),
                    );
                    self.tasks
                        .fail(&lease, self.current_step, failure.clone())?;
                    self.record_task_terminal_event(&task.task_id, "task.failed", Some(failure));
                    self.wake_tasks_waiting_on(&task.task_id)?;
                    completed += 1;
                    continue;
                }
            }
            self.tasks.complete(&lease, self.current_step)?;
            completed += 1;
        }
        Ok(completed)
    }
}
