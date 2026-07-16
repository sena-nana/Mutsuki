use std::sync::Arc;

use crossbeam_channel::{Sender, TrySendError, bounded};
use mutsuki_runtime_core::{RunnerManagementHandle, RuntimeResult};

use crate::actor::CoreActorMsg;
use crate::error::host_failure;

struct ManagementCommand {
    runner_id: String,
    invocation_id: String,
    handle: Arc<dyn RunnerManagementHandle>,
}

pub(crate) struct ManagementExecutor {
    sender: Sender<ManagementCommand>,
}

impl ManagementExecutor {
    pub(crate) fn new(
        thread_count: usize,
        queue_limit: usize,
        actor: std::sync::mpsc::Sender<CoreActorMsg>,
    ) -> RuntimeResult<Self> {
        if thread_count == 0 || queue_limit == 0 {
            return Err(host_failure(
                "host.worker.config",
                "management thread and queue limits must be greater than zero",
            ));
        }
        let (sender, receiver) = bounded::<ManagementCommand>(queue_limit);
        for index in 0..thread_count {
            let receiver = receiver.clone();
            let actor = actor.clone();
            std::thread::Builder::new()
                .name(format!("mutsuki-management-{index}"))
                .spawn(move || {
                    while let Ok(command) = receiver.recv() {
                        if command.handle.cancel(&command.invocation_id).is_err() {
                            let _ = actor.send(CoreActorMsg::ManagementFailed {
                                runner_id: command.runner_id,
                                invocation_id: command.invocation_id,
                            });
                        }
                    }
                })
                .map_err(|error| host_failure("host.management.spawn", error.to_string()))?;
        }
        Ok(Self { sender })
    }

    pub(crate) fn cancel(
        &self,
        runner_id: String,
        invocation_id: String,
        handle: Arc<dyn RunnerManagementHandle>,
    ) -> RuntimeResult<()> {
        self.sender
            .try_send(ManagementCommand {
                runner_id,
                invocation_id,
                handle,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    host_failure("host.management.saturated", "management queue is full")
                }
                TrySendError::Disconnected(_) => host_failure(
                    "host.management.disconnected",
                    "management executor is unavailable",
                ),
            })
    }
}
