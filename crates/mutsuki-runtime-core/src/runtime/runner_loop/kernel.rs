use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    DomainEvent, RunnerDescriptor, RuntimeError, RuntimeEventKind, StateDelta, Task,
};

use crate::{RuntimeFailure, RuntimeResult};

use super::CoreRuntime;

impl CoreRuntime {
    pub(super) fn process_kernel_tasks(
        &mut self,
        runner: &RunnerDescriptor,
        tasks: Vec<Task>,
    ) -> RuntimeResult<usize> {
        let mut completed = 0;
        for task in tasks {
            match task.protocol_id.as_str() {
                "core.commit" => {
                    let delta: StateDelta =
                        serde_json::from_value(task.payload.clone()).map_err(|err| {
                            RuntimeFailure::new(RuntimeError::new(
                                "state.delta_decode_failed",
                                "runtime.committer",
                                err.to_string(),
                            ))
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
                            RuntimeFailure::new(RuntimeError::new(
                                "event.decode_failed",
                                "runtime.event_log",
                                err.to_string(),
                            ))
                        })?;
                    self.events.record(
                        RuntimeEventKind::Task,
                        event.kind,
                        Some(event.event_id),
                        BTreeMap::new(),
                        None,
                    );
                }
                _ => {}
            }
            self.tasks.complete(&task.task_id, &runner.runner_id)?;
            completed += 1;
        }
        Ok(completed)
    }
}
