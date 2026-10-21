//! Task submission, inspection, cancellation and waiting through the Host actor.

#[allow(clippy::wildcard_imports)]
use super::*;

impl ServiceRuntimeInner {
    pub(crate) fn core_begin_drain(&self) -> ControlResponse {
        let guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_ref() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        match runtime.begin_drain() {
            Ok(state) => ControlResponse::ok(ControlResult::CoreBeginDrain(CoreDrainResponse {
                state: runtime_stop_state(state).to_owned(),
            })),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) fn task_submit_batch(&self, param: TaskSubmitBatchParam) -> ControlResponse {
        if param.batch.tasks.is_empty() {
            return ControlResponse::err(ControlError::BadRequest(
                "task batch must contain at least one task".into(),
            ));
        }
        let guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_ref() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        match runtime.dispatch(HostRuntimeCommand::SubmitBatch(Box::new(param.batch))) {
            Ok(HostRuntimeReply::TaskBatchSubmitted(handles)) => {
                ControlResponse::ok(ControlResult::TaskSubmitBatch(TaskSubmitBatchResponse {
                    handles,
                }))
            }
            Ok(other) => ControlResponse::err(ControlError::Failed(format!(
                "unexpected task submit reply: {other:?}"
            ))),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) fn task_list(&self) -> ControlResponse {
        let mut guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_mut() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        match runtime.task_snapshots() {
            Ok(snapshots) => ControlResponse::ok(ControlResult::TaskList(
                to_control_task_snapshots(snapshots),
            )),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) fn task_cancel(&self, param: IdParam) -> ControlResponse {
        let mut guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_mut() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        let handle = match resolve_task_handle(runtime, &param.id) {
            Ok(handle) => handle,
            Err(error) => return ControlResponse::err(error),
        };
        match runtime.dispatch(HostRuntimeCommand::CancelTask(handle)) {
            Ok(_) => ControlResponse::ok(ControlResult::TaskCancel),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) fn task_outcome(&self, param: IdParam) -> ControlResponse {
        let guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_ref() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        let handle = match resolve_task_handle(runtime, &param.id) {
            Ok(handle) => handle,
            Err(error) => return ControlResponse::err(error),
        };
        match runtime.dispatch(HostRuntimeCommand::TaskOutcome(handle.clone())) {
            Ok(HostRuntimeReply::TaskOutcome(outcome)) => ControlResponse::ok(
                ControlResult::TaskOutcome(to_control_task_outcome(&handle.task_id, outcome)),
            ),
            Ok(other) => ControlResponse::err(ControlError::Failed(format!(
                "unexpected task outcome reply: {other:?}"
            ))),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) fn task_outcomes_batch(&self, param: TaskOutcomesBatchParam) -> ControlResponse {
        if param.ids.is_empty() {
            return ControlResponse::err(ControlError::BadRequest(
                "task outcomes batch requires at least one id".into(),
            ));
        }
        let guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_ref() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        let handles = match resolve_task_handles(runtime, &param.ids) {
            Ok(handles) => handles,
            Err(error) => return ControlResponse::err(error),
        };
        match runtime.task_states(handles) {
            Ok(states) => ControlResponse::ok(ControlResult::TaskOutcomesBatch(
                TaskOutcomesBatchResponse {
                    outcomes: states
                        .into_iter()
                        .map(|state| to_control_task_outcome(&state.handle.task_id, state.outcome))
                        .collect(),
                },
            )),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) async fn task_wait(&self, param: TaskWaitParam) -> ControlResponse {
        if param.ids.is_empty() {
            return ControlResponse::err(ControlError::BadRequest(
                "task wait requires at least one id".into(),
            ));
        }
        if param.timeout_ms > mutsuki_service_control::MAX_TASK_WAIT_TIMEOUT_MS {
            return ControlResponse::err(ControlError::BadRequest(format!(
                "task wait timeout_ms must be at most {}",
                mutsuki_service_control::MAX_TASK_WAIT_TIMEOUT_MS
            )));
        }
        let (subscription, handles) = {
            let guard = self.host_runtime.lock().expect("host runtime mutex");
            let Some(runtime) = guard.as_ref() else {
                return ControlResponse::err(ControlError::CoreUnavailable);
            };
            let handles = match resolve_task_handles(runtime, &param.ids) {
                Ok(handles) => handles,
                Err(error) => return ControlResponse::err(error),
            };
            (runtime.subscribe_task_completions(), handles)
        };
        let timeout = std::time::Duration::from_millis(param.timeout_ms);
        let deadline = std::time::Instant::now() + timeout;
        let mut revision = subscription.revision();
        loop {
            let states = {
                let guard = self.host_runtime.lock().expect("host runtime mutex");
                let Some(runtime) = guard.as_ref() else {
                    return ControlResponse::err(ControlError::CoreUnavailable);
                };
                match runtime.task_states(handles.clone()) {
                    Ok(states) => states,
                    Err(error) => {
                        return ControlResponse::err(ControlError::Failed(error.to_string()));
                    }
                }
            };
            let all_terminal = states
                .iter()
                .all(|state| is_terminal_task_status(state.status.as_ref()));
            if all_terminal {
                return task_wait_response(states, false);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return task_wait_response(states, true);
            }
            // The completion wait is a condvar wait: run it on the blocking pool
            // so no async worker parks, and so the IPC connection can drop this
            // future (the detached wait ends at the capped deadline).
            let remaining = deadline.saturating_duration_since(now);
            let waiter = subscription.clone();
            let notified =
                tokio::task::spawn_blocking(move || waiter.wait_after_timeout(revision, remaining))
                    .await
                    .unwrap_or(None);
            match notified {
                Some(next_revision) => revision = next_revision,
                None => {
                    let guard = self.host_runtime.lock().expect("host runtime mutex");
                    let Some(runtime) = guard.as_ref() else {
                        return ControlResponse::err(ControlError::CoreUnavailable);
                    };
                    return match runtime.task_states(handles) {
                        Ok(states) => task_wait_response(states, true),
                        Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
                    };
                }
            }
        }
    }

    pub(crate) fn task_events_after(&self, param: TaskEventsAfterParam) -> ControlResponse {
        const MAX_EVENTS_PER_PAGE: usize = 1_024;

        if param.limit == 0 || param.limit > MAX_EVENTS_PER_PAGE {
            return ControlResponse::err(ControlError::BadRequest(format!(
                "event page limit must be in 1..={MAX_EVENTS_PER_PAGE}"
            )));
        }
        let guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_ref() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        let page = match runtime.events_after(param.sequence, param.limit) {
            Ok(page) => page,
            Err(error) => return ControlResponse::err(ControlError::Failed(error.to_string())),
        };
        ControlResponse::ok(ControlResult::TaskEventsAfter(TaskEventPage {
            next_sequence: page.next_sequence,
            earliest_available_sequence: page.earliest_available_sequence,
            latest_sequence: page.latest_sequence,
            lost: page.lost,
            dropped: page.dropped,
            has_more: page.truncated,
            events: page.items,
        }))
    }
}
