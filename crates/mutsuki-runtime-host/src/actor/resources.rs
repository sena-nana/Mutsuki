//! Actor-owned ordered provider lanes. Entries are bounded by existing Host limits.
use super::*;
use crate::async_executor::AsyncExecutorEvent;
use mutsuki_runtime_sdk::ResourceProviderOrdering;

pub(super) struct PendingResource {
    provider_id: String,
    payload_bytes: usize,
    message: CoreActorMsg,
}

pub(super) enum ResourceExecutionLease {
    Sync {
        _provider: Arc<dyn mutsuki_runtime_sdk::ResourceProviderGateway>,
    },
    Async {
        _provider: Arc<dyn mutsuki_runtime_sdk::AsyncResourceProviderGateway>,
    },
}

fn command(message: &CoreActorMsg) -> Option<&HostRuntimeCommand> {
    match message {
        CoreActorMsg::Command(command, _) | CoreActorMsg::AsyncResourceCommand(command, _) => {
            Some(command)
        }
        _ => None,
    }
}
fn fail(message: CoreActorMsg, code: &str, detail: &str) {
    let error = mutsuki_runtime_core::RuntimeFailure::new(
        mutsuki_runtime_contracts::RuntimeError::new(code, "host.resource", detail),
    );
    match message {
        CoreActorMsg::Command(_, reply) => {
            let _ = reply.send(Err(error));
        }
        CoreActorMsg::AsyncResourceCommand(_, reply) => {
            let _ = reply.send(Err(error));
        }
        _ => unreachable!(),
    }
}
pub(super) fn completed_resource(event: &AsyncExecutorEvent) -> Option<(String, String, bool)> {
    let (invocation, poisoned) = match event {
        AsyncExecutorEvent::ResourceCompleted { invocation, .. } => (invocation, false),
        AsyncExecutorEvent::ResourcePanicked { invocation, .. } => (invocation, true),
        // Deadline answers only the caller: the operation still owns the lane.
        _ => return None,
    };
    Some((
        invocation
            .runner_id
            .as_str()
            .strip_prefix("resource:")?
            .to_string(),
        invocation.invocation_id.clone(),
        poisoned,
    ))
}
impl CoreActor {
    /// Every actor drain path must release the lane only after lifecycle application.
    pub(super) fn apply_async_event(
        &mut self,
        event: AsyncExecutorEvent,
    ) -> RuntimeResult<RunnerLoopReport> {
        let completed = completed_resource(&event);
        let result = handle_async_event(
            event,
            &mut self.core,
            &mut self.pending_cancels,
            &mut self.running_batches_by_task,
            &mut self.draining_invocations,
            &mut self.offloaded_resource_replies,
        );
        self.finish_resource(completed);
        result
    }
    pub(super) fn track_resource_invocation(&mut self, id: &str, invocation_id: &str) {
        let lease = if let Some(provider) = self.config.async_resource_providers.get(id) {
            ResourceExecutionLease::Async {
                _provider: provider.clone(),
            }
        } else if let Some(provider) = self.config.resource_providers.get(id) {
            ResourceExecutionLease::Sync {
                _provider: provider.clone(),
            }
        } else {
            return;
        };
        self.resource_execution_leases
            .insert(invocation_id.to_string(), lease);
        *self.resource_inflight.entry(id.to_string()).or_default() += 1;
        if self.resource_ordered(id) {
            self.resource_busy.insert(id.to_string());
        }
    }
    pub(super) fn fence_reloaded_resource_providers(&mut self) {
        for id in self.resource_inflight.keys() {
            if self.resource_ordered(id) {
                self.resource_busy.insert(id.clone());
            }
        }
    }
    pub(super) fn resource_ordered(&self, id: &str) -> bool {
        self.config
            .resource_providers
            .get(id)
            .is_some_and(|p| p.ordering() == ResourceProviderOrdering::Ordered)
            || self
                .config
                .async_resource_providers
                .get(id)
                .is_some_and(|p| p.ordering() == ResourceProviderOrdering::Ordered)
    }
    pub(super) fn defer_resource_message(&mut self, message: CoreActorMsg) -> Option<CoreActorMsg> {
        let Some(cmd) = command(&message) else {
            return Some(message);
        };
        let Some(id) = resource_router::resource_command_provider(cmd) else {
            return Some(message);
        };
        if self.resource_shutdown {
            fail(
                message,
                mutsuki_runtime_contracts::ERR_RUNTIME_ABORTED,
                "host shutting down",
            );
            return None;
        }
        if self.resource_poisoned.contains(&id) {
            fail(
                message,
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "provider lane has unknown effects; restart required",
            );
            return None;
        }
        if self.resource_busy.contains(&id) {
            let bytes = resource_router::payload_bytes(cmd);
            if self.resource_pending.len() >= self.config.actor_data_queue_limit
                || bytes
                    > self
                        .config
                        .pool_max_inflight_bytes
                        .saturating_sub(self.resource_pending_bytes)
            {
                fail(
                    message,
                    mutsuki_runtime_contracts::ERR_CAPABILITY_EXHAUSTED,
                    "ordered provider queue capacity",
                );
            } else {
                self.resource_pending_bytes += bytes;
                self.resource_pending.push_back(PendingResource {
                    provider_id: id,
                    payload_bytes: bytes,
                    message,
                });
            }
            return None;
        }
        Some(message)
    }
    pub(super) fn reject_pending_resources(&mut self) {
        for pending in self.resource_pending.drain(..) {
            fail(
                pending.message,
                mutsuki_runtime_contracts::ERR_RUNTIME_ABORTED,
                "host shutting down",
            );
        }
        self.resource_pending_bytes = 0;
        self.resource_pending_ready = false;
    }
    pub(super) fn next_resource_message(&mut self) -> Option<CoreActorMsg> {
        // Enqueue only happens behind a busy lane. Do not scan a saturated
        // queue on every unrelated actor message; only completion can unblock it.
        if !self.resource_pending_ready {
            return None;
        }
        let Some(index) = self
            .resource_pending
            .iter()
            .position(|pending| !self.resource_busy.contains(&pending.provider_id))
        else {
            self.resource_pending_ready = false;
            return None;
        };
        let pending = self.resource_pending.remove(index)?;
        self.resource_pending_bytes -= pending.payload_bytes;
        Some(pending.message)
    }

    pub(super) fn finish_resource(&mut self, completed: Option<(String, String, bool)>) {
        if let Some((id, invocation_id, poisoned)) = completed {
            if self
                .resource_execution_leases
                .remove(&invocation_id)
                .is_some()
            {
                let count = self
                    .resource_inflight
                    .get_mut(&id)
                    .expect("tracked provider invocation");
                *count -= 1;
                if *count == 0 {
                    self.resource_inflight.remove(&id);
                    if self.resource_busy.remove(&id) {
                        self.resource_pending_ready = true;
                    }
                }
            }
            if poisoned {
                self.resource_poisoned.insert(id);
            }
        }
    }
}
