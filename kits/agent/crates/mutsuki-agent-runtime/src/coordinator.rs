use std::collections::BTreeSet;

use mutsuki_agent_contracts::{
    AgentAttemptOutcome, AgentBudget, AgentError, AgentSessionState, AgentSessionStatus,
    AgentStepState, AgentStepStatus, AgentTurnState, AgentTurnStatus, PendingApproval,
    PendingModelCall, PendingToolCall, PermissionDecision, PermissionDecisionKind, ResourceRef,
    SessionVersion, StopReason,
};

#[derive(Clone, Debug, Default)]
pub struct AgentSessionCoordinator;

impl AgentSessionCoordinator {
    pub fn start_session<B>(
        &self,
        session_id: impl Into<String>,
        profile_id: impl Into<String>,
        snapshot: ResourceRef,
        budget: B,
    ) -> Result<AgentSessionState, AgentError>
    where
        B: Into<AgentBudget>,
    {
        let session_id = session_id.into();
        let profile_id = profile_id.into();
        if session_id.trim().is_empty() || profile_id.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "session_id and profile_id are required",
            ));
        }
        Ok(AgentSessionState {
            session_id,
            profile_id,
            version: SessionVersion(1),
            status: AgentSessionStatus::Active,
            budget: budget.into(),
            usage: Default::default(),
            cost_microunits: 0,
            snapshot,
            turns: Vec::new(),
            pending_approvals: Vec::new(),
            pending_interactions: Vec::new(),
            completed_attempts: BTreeSet::new(),
            committed_side_effects: BTreeSet::new(),
        })
    }

    pub fn start_turn(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        turn_id: impl Into<String>,
    ) -> Result<SessionVersion, AgentError> {
        self.require_version(state, expected_version)?;
        if state.status != AgentSessionStatus::Active {
            return Err(AgentError::new(
                "agent.session.not_active",
                "only an active session may start a turn",
            ));
        }
        if state.turns.last().is_some_and(|turn| {
            !matches!(
                turn.status,
                AgentTurnStatus::Completed | AgentTurnStatus::Cancelled | AgentTurnStatus::Failed
            )
        }) {
            return Err(AgentError::new(
                "agent.turn.already_active",
                "a session may advance only one turn at a time",
            ));
        }
        let turn_id = turn_id.into();
        if turn_id.trim().is_empty() || state.turns.iter().any(|turn| turn.turn_id == turn_id) {
            return Err(AgentError::invalid_input(
                "turn_id must be non-empty and unique within the session",
            ));
        }
        state.turns.push(AgentTurnState {
            turn_id,
            status: AgentTurnStatus::Created,
            expected_version,
            steps: vec![AgentStepState {
                step_index: 0,
                status: AgentStepStatus::Pending,
                pending_model: None,
                pending_tools: Vec::new(),
            }],
            stop_reason: None,
        });
        Ok(self.bump_version(state))
    }

    pub fn complete_attempt(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        attempt_id: &str,
    ) -> Result<(SessionVersion, bool), AgentError> {
        if attempt_id.trim().is_empty() {
            return Err(AgentError::invalid_input("attempt_id is required"));
        }
        if state.completed_attempts.contains(attempt_id) {
            return Ok((state.version, false));
        }
        self.require_version(state, expected_version)?;
        state.completed_attempts.insert(attempt_id.to_owned());
        Ok((self.bump_version(state), true))
    }

    pub fn submit_model_call(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        pending: PendingModelCall,
    ) -> Result<SessionVersion, AgentError> {
        self.require_version(state, expected_version)?;
        let turn = active_turn_mut(state)?;
        let step = active_step_mut(turn)?;
        if pending.attempt_id.trim().is_empty()
            || pending.task_id.trim().is_empty()
            || pending.step_index != step.step_index
            || step.pending_model.is_some()
            || !step.pending_tools.is_empty()
        {
            return Err(AgentError::invalid_input(
                "model attempt must uniquely target the active empty step",
            ));
        }
        step.pending_model = Some(pending);
        step.status = AgentStepStatus::Submitted;
        turn.status = AgentTurnStatus::Generating;
        Ok(self.bump_version(state))
    }

    pub fn submit_tool_batch(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        pending: Vec<PendingToolCall>,
    ) -> Result<SessionVersion, AgentError> {
        self.require_version(state, expected_version)?;
        let turn = active_turn_mut(state)?;
        let step = active_step_mut(turn)?;
        let mut attempts = BTreeSet::new();
        if pending.is_empty()
            || step.pending_model.is_some()
            || !step.pending_tools.is_empty()
            || pending.iter().any(|call| {
                call.attempt_id.trim().is_empty()
                    || call.task_id.trim().is_empty()
                    || call.step_index != step.step_index
                    || !attempts.insert(call.attempt_id.as_str())
            })
        {
            return Err(AgentError::invalid_input(
                "tool batch must contain unique attempts for the active empty step",
            ));
        }
        step.pending_tools = pending;
        step.status = AgentStepStatus::Submitted;
        turn.status = AgentTurnStatus::RunningTools;
        Ok(self.bump_version(state))
    }

    pub fn request_approval(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        pending: PendingApproval,
    ) -> Result<SessionVersion, AgentError> {
        self.require_version(state, expected_version)?;
        if pending.request.session_id != state.session_id
            || pending.request.version != expected_version.0
            || state.pending_approvals.iter().any(|approval| {
                approval.request.turn_id == pending.request.turn_id
                    && approval.request.action_id == pending.request.action_id
            })
        {
            return Err(AgentError::new(
                "agent.approval.invalid_binding",
                "approval must bind the current session version and a unique action",
            ));
        }
        let turn = active_turn_mut(state)?;
        if turn.turn_id != pending.request.turn_id {
            return Err(AgentError::new(
                "agent.approval.invalid_binding",
                "approval does not belong to the active turn",
            ));
        }
        if !turn.steps.last().is_some_and(|step| {
            step.pending_tools
                .iter()
                .any(|tool| tool.call.call_id == pending.tool_call.call_id)
        }) {
            return Err(AgentError::new(
                "agent.approval.invalid_binding",
                "approval action does not bind an active tool call",
            ));
        }
        turn.status = AgentTurnStatus::WaitingApproval;
        state.status = AgentSessionStatus::WaitingApproval;
        state.pending_approvals.push(pending);
        Ok(self.bump_version(state))
    }

    pub fn mark_tool_side_effect_committed(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        attempt_id: &str,
    ) -> Result<(SessionVersion, bool), AgentError> {
        if state.committed_side_effects.contains(attempt_id) {
            return Ok((state.version, false));
        }
        self.require_version(state, expected_version)?;
        let turn = active_turn_mut(state)?;
        let step = active_step_mut(turn)?;
        let pending = step
            .pending_tools
            .iter_mut()
            .find(|pending| pending.attempt_id == attempt_id)
            .ok_or_else(|| AgentError::not_found("pending tool attempt was not found"))?;
        pending.side_effect_committed = true;
        state.committed_side_effects.insert(attempt_id.to_owned());
        Ok((self.bump_version(state), true))
    }

    pub fn finish_attempt(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        attempt_id: &str,
        outcome: AgentAttemptOutcome,
    ) -> Result<(SessionVersion, bool), AgentError> {
        if attempt_id.trim().is_empty() {
            return Err(AgentError::invalid_input("attempt_id is required"));
        }
        if state.completed_attempts.contains(attempt_id) {
            return Ok((state.version, false));
        }
        if let AgentAttemptOutcome::BudgetExceeded { reason } = &outcome
            && !matches!(
                reason,
                StopReason::MaxSteps | StopReason::TokenBudget | StopReason::CostBudget
            )
        {
            return Err(AgentError::invalid_input(
                "budget outcome requires a budget stop reason",
            ));
        }
        self.require_version(state, expected_version)?;
        let stop_reason = {
            let turn = active_turn_mut(state)?;
            let step = active_step_mut(turn)?;
            let matched_model = step
                .pending_model
                .as_ref()
                .is_some_and(|pending| pending.attempt_id == attempt_id);
            let tool_index = step
                .pending_tools
                .iter()
                .position(|pending| pending.attempt_id == attempt_id);
            if !matched_model && tool_index.is_none() {
                return Err(AgentError::not_found("pending attempt was not found"));
            }
            if matched_model {
                step.pending_model = None;
            }
            if let Some(index) = tool_index {
                step.pending_tools.remove(index);
            }
            match outcome {
                AgentAttemptOutcome::Succeeded => {
                    if step.pending_model.is_none() && step.pending_tools.is_empty() {
                        step.status = AgentStepStatus::Completed;
                        turn.status = AgentTurnStatus::Created;
                    } else {
                        step.status = AgentStepStatus::Waiting;
                    }
                    None
                }
                AgentAttemptOutcome::RetryableFailure { .. } => {
                    step.status = AgentStepStatus::Pending;
                    turn.status = AgentTurnStatus::Created;
                    None
                }
                AgentAttemptOutcome::Cancelled => Some(StopReason::Cancelled),
                AgentAttemptOutcome::DeadlineExceeded => Some(StopReason::Deadline),
                AgentAttemptOutcome::NonRetryableFailure { .. } => Some(if matched_model {
                    StopReason::ModelFailure
                } else {
                    StopReason::ToolFailure
                }),
                AgentAttemptOutcome::BudgetExceeded { reason } => Some(reason),
            }
        };
        state.completed_attempts.insert(attempt_id.to_owned());
        if let Some(reason) = stop_reason {
            apply_stop_reason(state, reason)?;
        } else {
            state.status = AgentSessionStatus::Active;
        }
        Ok((self.bump_version(state), true))
    }

    pub fn apply_permission(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        decision: PermissionDecision,
    ) -> Result<SessionVersion, AgentError> {
        self.require_version(state, expected_version)?;
        if decision.session_id != state.session_id {
            return Err(AgentError::new(
                "agent.approval.stale",
                "approval is not bound to the current session",
            ));
        }
        let index = state
            .pending_approvals
            .iter()
            .position(|pending| {
                pending.request.turn_id == decision.turn_id
                    && pending.request.action_id == decision.action_id
            })
            .ok_or_else(|| AgentError::not_found("pending approval was not found"))?;
        if decision.version != state.pending_approvals[index].request.version {
            return Err(AgentError::new(
                "agent.approval.stale",
                "approval decision version does not match the pending action",
            ));
        }
        state.pending_approvals.remove(index);
        match decision.decision {
            PermissionDecisionKind::Approved => {
                let turn = active_turn_mut(state)?;
                turn.status = if turn
                    .steps
                    .last()
                    .is_some_and(|step| !step.pending_tools.is_empty())
                {
                    AgentTurnStatus::RunningTools
                } else {
                    AgentTurnStatus::Created
                };
                state.status = AgentSessionStatus::Active;
            }
            PermissionDecisionKind::Rejected => {
                apply_stop_reason(state, StopReason::ApprovalRejected)?;
            }
            PermissionDecisionKind::TimedOut => {
                apply_stop_reason(state, StopReason::ApprovalTimedOut)?;
            }
            PermissionDecisionKind::Cancelled => {
                apply_stop_reason(state, StopReason::Cancelled)?;
            }
        }
        Ok(self.bump_version(state))
    }

    pub fn stop_turn(
        &self,
        state: &mut AgentSessionState,
        expected_version: SessionVersion,
        reason: StopReason,
    ) -> Result<SessionVersion, AgentError> {
        self.require_version(state, expected_version)?;
        let turn = state
            .turns
            .last_mut()
            .ok_or_else(|| AgentError::not_found("active turn was not found"))?;
        turn.stop_reason = Some(reason.clone());
        turn.status = match reason {
            StopReason::Completed => AgentTurnStatus::Completed,
            StopReason::Cancelled | StopReason::ApprovalRejected | StopReason::ApprovalTimedOut => {
                AgentTurnStatus::Cancelled
            }
            _ => AgentTurnStatus::Failed,
        };
        state.status = match turn.status {
            AgentTurnStatus::Cancelled => AgentSessionStatus::Cancelled,
            AgentTurnStatus::Failed => AgentSessionStatus::Failed,
            _ => AgentSessionStatus::Active,
        };
        Ok(self.bump_version(state))
    }

    fn require_version(
        &self,
        state: &AgentSessionState,
        expected: SessionVersion,
    ) -> Result<(), AgentError> {
        if state.version != expected {
            return Err(AgentError::new(
                "agent.session.version_conflict",
                format!(
                    "expected session version {}, actual {}",
                    expected.0, state.version.0
                ),
            ));
        }
        Ok(())
    }

    fn bump_version(&self, state: &mut AgentSessionState) -> SessionVersion {
        state.version.0 = state.version.0.saturating_add(1);
        state.version
    }
}

fn active_turn_mut(state: &mut AgentSessionState) -> Result<&mut AgentTurnState, AgentError> {
    state
        .turns
        .last_mut()
        .filter(|turn| {
            !matches!(
                turn.status,
                AgentTurnStatus::Completed | AgentTurnStatus::Cancelled | AgentTurnStatus::Failed
            )
        })
        .ok_or_else(|| AgentError::not_found("active turn was not found"))
}

fn active_step_mut(turn: &mut AgentTurnState) -> Result<&mut AgentStepState, AgentError> {
    turn.steps
        .last_mut()
        .filter(|step| {
            !matches!(
                step.status,
                AgentStepStatus::Cancelled | AgentStepStatus::Failed
            )
        })
        .ok_or_else(|| AgentError::not_found("active step was not found"))
}

fn apply_stop_reason(state: &mut AgentSessionState, reason: StopReason) -> Result<(), AgentError> {
    let turn = active_turn_mut(state)?;
    turn.stop_reason = Some(reason.clone());
    turn.status = match reason {
        StopReason::Completed => AgentTurnStatus::Completed,
        StopReason::Cancelled | StopReason::ApprovalRejected | StopReason::ApprovalTimedOut => {
            AgentTurnStatus::Cancelled
        }
        _ => AgentTurnStatus::Failed,
    };
    if let Some(step) = turn.steps.last_mut() {
        step.status = match turn.status {
            AgentTurnStatus::Cancelled => AgentStepStatus::Cancelled,
            AgentTurnStatus::Failed => AgentStepStatus::Failed,
            _ => AgentStepStatus::Completed,
        };
        step.pending_model = None;
        step.pending_tools.clear();
    }
    state.status = match turn.status {
        AgentTurnStatus::Cancelled => AgentSessionStatus::Cancelled,
        AgentTurnStatus::Failed => AgentSessionStatus::Failed,
        AgentTurnStatus::Completed => AgentSessionStatus::Active,
        _ => state.status.clone(),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::{
        AgentToolCall, PermissionDecisionKind, PermissionRequest, ToolSideEffect,
    };
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState,
        ResourceSemantic,
    };

    fn snapshot() -> ResourceRef {
        ResourceRef {
            ref_id: "session:snapshot:1".into(),
            resource_id: ResourceId {
                kind_id: "agent.session.snapshot".into(),
                slot_id: "1".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::VersionedSnapshot,
            provider_id: "test.provider".into(),
            resource_kind: "agent.session.snapshot".into(),
            schema: "mutsuki.agent.session.snapshot@1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: Some(0),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    #[test]
    fn optimistic_version_and_attempt_fence_prevent_double_continuation() {
        let coordinator = AgentSessionCoordinator;
        let mut state = coordinator
            .start_session("session", "profile", snapshot(), AgentBudget::default())
            .unwrap();
        let version = coordinator
            .start_turn(&mut state, SessionVersion(1), "turn-1")
            .unwrap();
        let (version, first) = coordinator
            .complete_attempt(&mut state, version, "attempt-1")
            .unwrap();
        let (same_version, duplicate) = coordinator
            .complete_attempt(&mut state, version, "attempt-1")
            .unwrap();
        assert!(first);
        assert!(!duplicate);
        assert_eq!(same_version, version);
    }

    fn tool(call_id: &str, attempt_id: &str) -> PendingToolCall {
        PendingToolCall {
            attempt_id: attempt_id.into(),
            task_id: format!("task:{attempt_id}"),
            step_index: 0,
            call: AgentToolCall {
                call_id: call_id.into(),
                name: "workspace.write".into(),
                input: serde_json::json!({"path": "fixture.txt"}),
            },
            side_effect_committed: false,
        }
    }

    #[test]
    fn tool_batch_and_side_effect_fences_are_idempotent() {
        let coordinator = AgentSessionCoordinator;
        let mut state = coordinator
            .start_session("session", "profile", snapshot(), AgentBudget::default())
            .unwrap();
        let version = coordinator
            .start_turn(&mut state, SessionVersion(1), "turn")
            .unwrap();
        let version = coordinator
            .submit_tool_batch(
                &mut state,
                version,
                vec![tool("call-1", "attempt-1"), tool("call-2", "attempt-2")],
            )
            .unwrap();
        let (version, committed) = coordinator
            .mark_tool_side_effect_committed(&mut state, version, "attempt-1")
            .unwrap();
        let (same_version, duplicate) = coordinator
            .mark_tool_side_effect_committed(&mut state, SessionVersion(1), "attempt-1")
            .unwrap();
        assert!(committed);
        assert!(!duplicate);
        assert_eq!(same_version, version);

        let (version, accepted) = coordinator
            .finish_attempt(
                &mut state,
                version,
                "attempt-1",
                AgentAttemptOutcome::Succeeded,
            )
            .unwrap();
        assert!(accepted);
        assert_eq!(state.turns[0].steps[0].status, AgentStepStatus::Waiting);
        let (_, accepted) = coordinator
            .finish_attempt(
                &mut state,
                version,
                "attempt-2",
                AgentAttemptOutcome::Succeeded,
            )
            .unwrap();
        assert!(accepted);
        assert_eq!(state.turns[0].steps[0].status, AgentStepStatus::Completed);
    }

    #[test]
    fn approval_decision_is_bound_to_action_version_and_stops_rejected_turn() {
        let coordinator = AgentSessionCoordinator;
        let mut state = coordinator
            .start_session("session", "profile", snapshot(), AgentBudget::default())
            .unwrap();
        let version = coordinator
            .start_turn(&mut state, SessionVersion(1), "turn")
            .unwrap();
        let pending_tool = tool("call-1", "attempt-1");
        let version = coordinator
            .submit_tool_batch(&mut state, version, vec![pending_tool.clone()])
            .unwrap();
        let action_version = version.0;
        let version = coordinator
            .request_approval(
                &mut state,
                version,
                PendingApproval {
                    request: PermissionRequest {
                        session_id: "session".into(),
                        turn_id: "turn".into(),
                        action_id: "action-1".into(),
                        tool: "workspace.write".into(),
                        side_effect: ToolSideEffect::WorkspaceWrite,
                        summary: "write fixture".into(),
                        version: action_version,
                    },
                    tool_call: pending_tool.call,
                },
            )
            .unwrap();
        assert_eq!(state.status, AgentSessionStatus::WaitingApproval);
        let version = coordinator
            .apply_permission(
                &mut state,
                version,
                PermissionDecision {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    action_id: "action-1".into(),
                    version: action_version,
                    decision: PermissionDecisionKind::Rejected,
                },
            )
            .unwrap();
        assert_eq!(version, state.version);
        assert_eq!(state.status, AgentSessionStatus::Cancelled);
        assert_eq!(
            state.turns[0].stop_reason,
            Some(StopReason::ApprovalRejected)
        );
    }

    #[test]
    fn terminal_attempt_outcomes_are_deterministic() {
        let cases = [
            (AgentAttemptOutcome::Cancelled, StopReason::Cancelled),
            (AgentAttemptOutcome::DeadlineExceeded, StopReason::Deadline),
            (
                AgentAttemptOutcome::NonRetryableFailure {
                    code: "provider.bad_request".into(),
                },
                StopReason::ModelFailure,
            ),
            (
                AgentAttemptOutcome::BudgetExceeded {
                    reason: StopReason::TokenBudget,
                },
                StopReason::TokenBudget,
            ),
        ];
        for (index, (outcome, expected_reason)) in cases.into_iter().enumerate() {
            let coordinator = AgentSessionCoordinator;
            let mut state = coordinator
                .start_session(
                    format!("session-{index}"),
                    "profile",
                    snapshot(),
                    AgentBudget::default(),
                )
                .unwrap();
            let version = coordinator
                .start_turn(&mut state, SessionVersion(1), "turn")
                .unwrap();
            let version = coordinator
                .submit_model_call(
                    &mut state,
                    version,
                    PendingModelCall {
                        attempt_id: "model-attempt".into(),
                        task_id: "model-task".into(),
                        step_index: 0,
                    },
                )
                .unwrap();
            coordinator
                .finish_attempt(&mut state, version, "model-attempt", outcome)
                .unwrap();
            assert_eq!(state.turns[0].stop_reason, Some(expected_reason));
        }
    }

    #[test]
    fn one_hundred_parallel_sessions_do_not_share_state() {
        let workers = (0..100)
            .map(|index| {
                std::thread::spawn(move || {
                    let coordinator = AgentSessionCoordinator;
                    let session_id = format!("session-{index}");
                    let attempt_id = format!("attempt-{index}");
                    let mut state = coordinator
                        .start_session(
                            session_id.clone(),
                            "profile",
                            snapshot(),
                            AgentBudget::default(),
                        )
                        .unwrap();
                    let version = coordinator
                        .start_turn(&mut state, SessionVersion(1), "turn")
                        .unwrap();
                    let version = coordinator
                        .submit_model_call(
                            &mut state,
                            version,
                            PendingModelCall {
                                attempt_id: attempt_id.clone(),
                                task_id: format!("task-{index}"),
                                step_index: 0,
                            },
                        )
                        .unwrap();
                    coordinator
                        .finish_attempt(
                            &mut state,
                            version,
                            &attempt_id,
                            AgentAttemptOutcome::Succeeded,
                        )
                        .unwrap();
                    (session_id, attempt_id, state)
                })
            })
            .collect::<Vec<_>>();

        let states = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(states.len(), 100);
        for (session_id, attempt_id, state) in states {
            assert_eq!(state.session_id, session_id);
            assert_eq!(state.completed_attempts, BTreeSet::from([attempt_id]));
            assert_eq!(state.turns.len(), 1);
        }
    }
}
