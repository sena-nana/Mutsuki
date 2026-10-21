//! Mapping Host/Core snapshots into public control-plane views.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn surface_compatibility(compatibility: SurfaceCompatibility) -> String {
    match compatibility {
        SurfaceCompatibility::Identical => "identical",
        SurfaceCompatibility::Additive => "additive",
        SurfaceCompatibility::Deprecated => "deprecated",
        SurfaceCompatibility::Removed => "removed",
        SurfaceCompatibility::Breaking => "breaking",
    }
    .into()
}

pub(crate) fn to_control_runner_status(snapshots: Vec<RunnerSnapshot>) -> Vec<ControlRunnerStatus> {
    snapshots
        .into_iter()
        .map(|snapshot| ControlRunnerStatus {
            runner_id: snapshot.runner_id,
            plugin_id: snapshot.plugin_id,
            state: match snapshot.state {
                RunnerProcessState::Running => "running".into(),
                RunnerProcessState::Exited(code) => format!("exited:{code}"),
                RunnerProcessState::Failed => "failed".into(),
                RunnerProcessState::Stopped => "stopped".into(),
            },
            pid: snapshot.pid,
            restarts: snapshot.restarts,
            last_error: snapshot.last_error,
        })
        .collect()
}

pub(crate) fn to_control_task_snapshots(
    snapshots: Vec<HostTaskSnapshot>,
) -> Vec<ControlTaskSnapshot> {
    snapshots
        .into_iter()
        .map(|snapshot| ControlTaskSnapshot {
            task_id: snapshot.task_id.into(),
            protocol_id: snapshot.protocol_id.into(),
            status: task_status_name(&snapshot.status).into(),
            priority: snapshot.priority,
            ready_at_step: snapshot.ready_at_step,
            created_sequence: snapshot.created_sequence,
            registry_generation: snapshot.registry_generation,
            target_binding_id: snapshot.target_binding_id.map(Into::into),
            runner_hint: snapshot.runner_hint,
            claimed_by: snapshot.claimed_by,
            owner_runner: snapshot.owner_runner,
            lease_id: snapshot.lease_id.map(Into::into),
            trace_id: snapshot.trace_id.map(Into::into),
            correlation_id: snapshot.correlation_id,
            input_refs: snapshot.input_refs.into_iter().map(Into::into).collect(),
            output_ref: snapshot.output_ref.map(Into::into),
            continuation_ref: snapshot.continuation_ref.map(Into::into),
            required_surfaces: snapshot
                .required_surfaces
                .into_iter()
                .map(Into::into)
                .collect(),
            failure: snapshot.failure.map(|failure| ControlTaskFailureSummary {
                code: failure.code,
                source: failure.source,
                route: failure.route,
            }),
        })
        .collect()
}

pub(crate) fn to_control_task_outcome(
    task_id: &TaskId,
    outcome: Option<TaskOutcome>,
) -> TaskOutcomeView {
    match outcome {
        None => TaskOutcomeView {
            task_id: task_id.clone(),
            status: "pending".into(),
            output: None,
            output_ref: None,
            reason: None,
            error_code: None,
            evidence: BTreeMap::new(),
        },
        Some(TaskOutcome::Completed {
            task_id,
            output,
            output_ref,
        }) => TaskOutcomeView {
            task_id,
            status: "completed".into(),
            output,
            output_ref,
            reason: None,
            error_code: None,
            evidence: BTreeMap::new(),
        },
        Some(TaskOutcome::Failed { task_id, error }) => {
            let evidence = error
                .evidence
                .into_iter()
                .filter_map(|(key, value)| {
                    serde_json::to_value(value).ok().map(|value| (key, value))
                })
                .collect();
            TaskOutcomeView {
                task_id,
                status: "failed".into(),
                output: None,
                output_ref: None,
                reason: Some(error.route),
                error_code: Some(error.code),
                evidence,
            }
        }
        Some(TaskOutcome::Cancelled { task_id, reason }) => TaskOutcomeView {
            task_id,
            status: "cancelled".into(),
            output: None,
            output_ref: None,
            reason,
            error_code: None,
            evidence: BTreeMap::new(),
        },
        Some(TaskOutcome::Expired { task_id, reason }) => TaskOutcomeView {
            task_id,
            status: "expired".into(),
            output: None,
            output_ref: None,
            reason,
            error_code: None,
            evidence: BTreeMap::new(),
        },
        Some(TaskOutcome::DeadLetter { task_id, reason }) => TaskOutcomeView {
            task_id,
            status: "dead_letter".into(),
            output: None,
            output_ref: None,
            reason,
            error_code: None,
            evidence: BTreeMap::new(),
        },
    }
}

pub(crate) fn task_status_name(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Created => "created",
        TaskStatus::Ready => "ready",
        TaskStatus::Running => "running",
        TaskStatus::Waiting => "waiting",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Expired => "expired",
        TaskStatus::DeadLetter => "dead_letter",
    }
}

pub(crate) fn runtime_stop_state(state: RuntimeStopState) -> &'static str {
    match state {
        RuntimeStopState::Running => "running",
        RuntimeStopState::Draining => "draining",
        RuntimeStopState::Aborted => "aborted",
    }
}

pub(crate) fn resolve_task_handle(
    runtime: &HostRuntime,
    task_id: &str,
) -> Result<TaskHandle, ControlError> {
    let id = task_id.to_owned();
    let mut handles = resolve_task_handles(runtime, std::slice::from_ref(&id))?;
    Ok(handles.remove(0))
}

pub(crate) fn resolve_task_handles(
    runtime: &HostRuntime,
    task_ids: &[String],
) -> Result<Vec<TaskHandle>, ControlError> {
    let snapshots = runtime
        .task_snapshots()
        .map_err(|error| ControlError::Failed(error.to_string()))?;
    let mut by_id = snapshots
        .into_iter()
        .map(|snapshot| (snapshot.task_id.clone(), snapshot))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut handles = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        let Some(snapshot) = by_id.remove(task_id.as_str()) else {
            return Err(ControlError::Failed(format!(
                "task {task_id} was not found"
            )));
        };
        handles.push(TaskHandle {
            task_id: snapshot.task_id.into(),
            protocol_id: snapshot.protocol_id.into(),
            target_binding_id: snapshot.target_binding_id.map(Into::into),
            cancel_policy: CancelPolicy::Cascade,
            trace_id: snapshot.trace_id.map(Into::into),
            correlation_id: snapshot.correlation_id,
        });
    }
    Ok(handles)
}

pub(crate) fn is_terminal_task_status(status: Option<&TaskStatus>) -> bool {
    matches!(
        status,
        Some(
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::Expired
                | TaskStatus::DeadLetter
        )
    )
}

pub(crate) fn task_wait_response(states: Vec<HostTaskState>, timed_out: bool) -> ControlResponse {
    ControlResponse::ok(ControlResult::TaskWait(TaskWaitResponse {
        outcomes: states
            .into_iter()
            .map(|state| to_control_task_outcome(&state.handle.task_id, state.outcome))
            .collect(),
        timed_out,
    }))
}
