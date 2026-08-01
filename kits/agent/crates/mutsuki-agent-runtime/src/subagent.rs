//! SubAgent orchestration: agent-as-tool, handoff, parallel aggregation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentDelegationRequest, AgentError, AgentHandoffRequest, AgentMessage, AgentResult,
    AgentRunRequest, AgentRunResult, AgentRunStatus, AggregationPolicy, ChildAgentRunRef,
    DelegationBudget, DelegationMode, DelegationScope, HandoffRecord, ParallelDelegationRequest,
    ParallelDelegationResult, SubAgentDescriptor, SubAgentOutcomeKind, SubAgentResult,
};
use mutsuki_agent_sdk::{memory_resource_ref, session_resource_ref};
use serde_json::{Value, json};

const OWNER_ID: &str = "mutsuki.agent.runtime.subagent";
const DEFAULT_MAX_DEPTH: u32 = 4;

pub trait ChildAgentExecutor: Send + Sync {
    fn execute_child(&self, request: AgentRunRequest) -> AgentResult<AgentRunResult>;
}

/// Deterministic in-process executor used by conformance and unit tests.
#[derive(Clone, Default)]
pub struct EchoChildExecutor;

impl ChildAgentExecutor for EchoChildExecutor {
    fn execute_child(&self, request: AgentRunRequest) -> AgentResult<AgentRunResult> {
        let summary = request
            .messages
            .last()
            .map(|message| message.content.clone())
            .unwrap_or_default();
        Ok(AgentRunResult {
            status: AgentRunStatus::Completed,
            messages: vec![AgentMessage::assistant(format!(
                "child[{}]:{summary}",
                request.profile_id
            ))],
            steps: Vec::new(),
            usage: Default::default(),
            cost_microunits: 0,
            output_resource: None,
            pending_approvals: Vec::new(),
            events: Vec::new(),
        })
    }
}

#[derive(Clone)]
pub struct SubAgentOrchestrator {
    inner: Arc<SubAgentInner>,
}

struct SubAgentInner {
    next_id: AtomicU64,
    agents: Mutex<BTreeMap<String, SubAgentDescriptor>>,
    parent_scope: Mutex<BTreeMap<String, DelegationScope>>,
    parent_budget: Mutex<BTreeMap<String, DelegationBudget>>,
    children: Mutex<BTreeMap<String, Vec<ChildAgentRunRef>>>,
    completed_attempts: Mutex<BTreeSet<String>>,
    committed_side_effects: Mutex<BTreeSet<String>>,
    cancelled_parents: Mutex<BTreeSet<String>>,
    handoffs: Mutex<BTreeMap<String, Vec<HandoffRecord>>>,
    executor: Arc<dyn ChildAgentExecutor>,
}

impl SubAgentOrchestrator {
    pub fn new(executor: Arc<dyn ChildAgentExecutor>) -> Self {
        Self {
            inner: Arc::new(SubAgentInner {
                next_id: AtomicU64::new(1),
                agents: Mutex::new(BTreeMap::new()),
                parent_scope: Mutex::new(BTreeMap::new()),
                parent_budget: Mutex::new(BTreeMap::new()),
                children: Mutex::new(BTreeMap::new()),
                completed_attempts: Mutex::new(BTreeSet::new()),
                committed_side_effects: Mutex::new(BTreeSet::new()),
                cancelled_parents: Mutex::new(BTreeSet::new()),
                handoffs: Mutex::new(BTreeMap::new()),
                executor,
            }),
        }
    }

    pub fn register_agent(&self, descriptor: SubAgentDescriptor) -> AgentResult<()> {
        if descriptor.agent_id.trim().is_empty() || descriptor.profile_id.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "subagent id and profile_id are required",
            ));
        }
        self.inner
            .agents
            .lock()
            .expect("subagent mutex")
            .insert(descriptor.agent_id.clone(), descriptor);
        Ok(())
    }

    pub fn bind_parent(
        &self,
        parent_session_id: impl Into<String>,
        scope: DelegationScope,
        budget: DelegationBudget,
    ) {
        let parent_session_id = parent_session_id.into();
        self.inner
            .parent_scope
            .lock()
            .expect("subagent mutex")
            .insert(parent_session_id.clone(), scope);
        self.inner
            .parent_budget
            .lock()
            .expect("subagent mutex")
            .insert(parent_session_id, budget);
    }

    pub fn cancel_parent(&self, parent_session_id: &str) {
        self.inner
            .cancelled_parents
            .lock()
            .expect("subagent mutex")
            .insert(parent_session_id.to_owned());
    }

    pub fn invoke_as_tool(&self, request: AgentDelegationRequest) -> AgentResult<SubAgentResult> {
        let mut request = request;
        request.mode = DelegationMode::AgentAsTool;
        self.delegate(request)
    }

    pub fn handoff(&self, request: AgentHandoffRequest) -> AgentResult<HandoffRecord> {
        if request.session_id.trim().is_empty()
            || request.from_profile_id.trim().is_empty()
            || request.to_profile_id.trim().is_empty()
        {
            return Err(AgentError::invalid_input(
                "handoff requires session and profile ids",
            ));
        }
        if request.from_profile_id == request.to_profile_id {
            return Err(AgentError::invalid_input(
                "handoff target profile must differ from current profile",
            ));
        }
        let mut chain = request.chain;
        if chain.is_empty() {
            chain.push(request.from_profile_id.clone());
        }
        if chain.contains(&request.to_profile_id) {
            return Err(AgentError::new(
                "agent.subagent.handoff_cycle",
                "handoff chain would cycle",
            ));
        }
        if chain.len() as u32 >= DEFAULT_MAX_DEPTH {
            return Err(AgentError::new(
                "agent.subagent.depth_exceeded",
                "handoff depth budget exceeded",
            ));
        }
        let record = HandoffRecord {
            from_profile_id: request.from_profile_id,
            to_profile_id: request.to_profile_id.clone(),
            at_unix_ms: 0,
            reason: request.reason,
        };
        self.inner
            .handoffs
            .lock()
            .expect("subagent mutex")
            .entry(request.session_id)
            .or_default()
            .push(record.clone());
        Ok(record)
    }

    pub fn handoff_chain(&self, session_id: &str) -> Vec<HandoffRecord> {
        self.inner
            .handoffs
            .lock()
            .expect("subagent mutex")
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn delegate_parallel(
        &self,
        request: ParallelDelegationRequest,
    ) -> AgentResult<ParallelDelegationResult> {
        if request.children.is_empty() {
            return Err(AgentError::invalid_input(
                "parallel delegation requires at least one child",
            ));
        }
        if request.children.len() > 32 {
            return Err(AgentError::invalid_input(
                "parallel delegation child count exceeds hard limit",
            ));
        }
        let cancelled = self
            .inner
            .cancelled_parents
            .lock()
            .expect("subagent mutex")
            .contains(&request.parent_session_id);
        if cancelled {
            return Ok(ParallelDelegationResult {
                policy: request.policy,
                results: Vec::new(),
                completed: false,
                cancelled: true,
            });
        }

        let mut results = Vec::with_capacity(request.children.len());
        for mut child in request.children {
            child.parent_session_id = request.parent_session_id.clone();
            child.parent_turn_id = request.parent_turn_id.clone();
            child.budget = request.budget.intersect(&child.budget);
            child.mode = DelegationMode::Parallel;
            let result = self.delegate(child)?;
            let success = matches!(result.kind, SubAgentOutcomeKind::Succeeded);
            results.push(result);
            match &request.policy {
                AggregationPolicy::FirstSuccess if success => {
                    return Ok(ParallelDelegationResult {
                        policy: request.policy,
                        results,
                        completed: true,
                        cancelled: false,
                    });
                }
                AggregationPolicy::Quorum { required }
                    if results
                        .iter()
                        .filter(|item| matches!(item.kind, SubAgentOutcomeKind::Succeeded))
                        .count()
                        >= *required as usize =>
                {
                    return Ok(ParallelDelegationResult {
                        policy: request.policy,
                        results,
                        completed: true,
                        cancelled: false,
                    });
                }
                _ => {}
            }
        }
        let completed = match &request.policy {
            AggregationPolicy::All => results
                .iter()
                .all(|item| matches!(item.kind, SubAgentOutcomeKind::Succeeded)),
            AggregationPolicy::FirstSuccess => results
                .iter()
                .any(|item| matches!(item.kind, SubAgentOutcomeKind::Succeeded)),
            AggregationPolicy::Quorum { required } => {
                results
                    .iter()
                    .filter(|item| matches!(item.kind, SubAgentOutcomeKind::Succeeded))
                    .count()
                    >= *required as usize
            }
        };
        Ok(ParallelDelegationResult {
            policy: request.policy,
            results,
            completed,
            cancelled: false,
        })
    }

    pub fn restore_completed_attempt(&self, attempt_id: impl Into<String>) {
        self.inner
            .completed_attempts
            .lock()
            .expect("subagent mutex")
            .insert(attempt_id.into());
    }

    pub fn children_of(&self, parent_session_id: &str) -> Vec<ChildAgentRunRef> {
        self.inner
            .children
            .lock()
            .expect("subagent mutex")
            .get(parent_session_id)
            .cloned()
            .unwrap_or_default()
    }

    fn delegate(&self, request: AgentDelegationRequest) -> AgentResult<SubAgentResult> {
        validate_delegation(&request)?;
        if self
            .inner
            .cancelled_parents
            .lock()
            .expect("subagent mutex")
            .contains(&request.parent_session_id)
        {
            return Ok(cancelled_result(&request, "parent session cancelled"));
        }

        let agent = self
            .inner
            .agents
            .lock()
            .expect("subagent mutex")
            .get(&request.child_agent_id)
            .cloned()
            .ok_or_else(|| {
                AgentError::not_found(format!(
                    "subagent `{}` is not registered",
                    request.child_agent_id
                ))
            })?;
        if agent.profile_id != request.child_profile_id {
            return Err(AgentError::invalid_input(
                "child profile_id does not match registered subagent descriptor",
            ));
        }
        let max_depth = if agent.max_depth == 0 {
            DEFAULT_MAX_DEPTH
        } else {
            agent.max_depth
        };
        if request.depth >= max_depth {
            return Err(AgentError::new(
                "agent.subagent.depth_exceeded",
                "delegation depth budget exceeded",
            ));
        }

        let parent_scope = self
            .inner
            .parent_scope
            .lock()
            .expect("subagent mutex")
            .get(&request.parent_session_id)
            .cloned()
            .unwrap_or_else(|| DelegationScope {
                allowed_tools: agent.tools.clone(),
                allowed_permissions: agent.permissions.clone(),
                inherit_conversation: false,
                inherit_knowledge: false,
            });
        let effective_scope = parent_scope.intersect(&request.scope);
        if !request.scope.allowed_tools.is_empty() && effective_scope.allowed_tools.is_empty() {
            return Err(AgentError::new(
                "agent.subagent.scope_denied",
                "child tools are outside the parent scope intersection",
            ));
        }

        let parent_budget = self
            .inner
            .parent_budget
            .lock()
            .expect("subagent mutex")
            .get(&request.parent_session_id)
            .cloned()
            .unwrap_or_default();
        let budget = parent_budget.intersect(&request.budget);
        let max_child_steps = budget.max_child_steps;

        let seq = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let child_session_id = format!(
            "{}:child:{seq}:{}",
            request.parent_session_id, request.child_agent_id
        );
        let attempt_id = format!("attempt-{seq}");
        let child_run = ChildAgentRunRef {
            child_run_id: format!("child-run-{seq}"),
            parent_session_id: request.parent_session_id.clone(),
            child_session_id: child_session_id.clone(),
            child_profile_id: request.child_profile_id.clone(),
            attempt_id: attempt_id.clone(),
            depth: request.depth.saturating_add(1),
        };

        if !self
            .inner
            .completed_attempts
            .lock()
            .expect("subagent mutex")
            .insert(attempt_id.clone())
        {
            return Ok(SubAgentResult {
                child_run,
                kind: SubAgentOutcomeKind::Succeeded,
                summary: "duplicate child attempt suppressed by fencing".into(),
                details: None,
                run: None,
                partial: false,
            });
        }

        self.inner
            .children
            .lock()
            .expect("subagent mutex")
            .entry(request.parent_session_id.clone())
            .or_default()
            .push(child_run.clone());

        let mut run = AgentRunRequest::new(
            request.child_profile_id.clone(),
            messages_from_input(&request.input),
        );
        run.session_id = Some(child_session_id);
        run.budget = budget.into_run_budget();
        if let Some(max_steps) = max_child_steps {
            run.max_steps = max_steps;
        }
        run.metadata = Some(json!({
            "parent_session_id": request.parent_session_id,
            "parent_turn_id": request.parent_turn_id,
            "mode": request.mode,
            "allowed_tools": effective_scope.allowed_tools,
            "allowed_permissions": effective_scope.allowed_permissions,
            "input_ref": request.input_ref,
        }));

        let side_effect_key = format!("{}:side-effect", child_run.child_run_id);
        let run_result = match self.inner.executor.execute_child(run) {
            Ok(result) => result,
            Err(error) => {
                // Failed child must not poison parent completed-attempt fence for recovery retries
                // of a different attempt id; keep this attempt marked to avoid double side effects.
                return Ok(SubAgentResult {
                    child_run,
                    kind: SubAgentOutcomeKind::Failed,
                    summary: error.message,
                    details: None,
                    run: None,
                    partial: false,
                });
            }
        };

        if !self
            .inner
            .committed_side_effects
            .lock()
            .expect("subagent mutex")
            .insert(side_effect_key.clone())
        {
            return Ok(SubAgentResult {
                child_run,
                kind: SubAgentOutcomeKind::Succeeded,
                summary: "child side effect already committed".into(),
                details: Some(session_resource_ref(OWNER_ID, &side_effect_key)),
                run: None,
                partial: false,
            });
        }

        let summary = run_result
            .messages
            .last()
            .map(|message| message.content.clone())
            .unwrap_or_else(|| "child completed".into());
        let details = Some(memory_resource_ref(
            OWNER_ID,
            format!("{}:details", child_run.child_run_id),
        ));
        let kind = match run_result.status {
            AgentRunStatus::Completed => SubAgentOutcomeKind::Succeeded,
            AgentRunStatus::Cancelled => SubAgentOutcomeKind::Cancelled,
            AgentRunStatus::BudgetExceeded => SubAgentOutcomeKind::BudgetExceeded,
            AgentRunStatus::Failed => SubAgentOutcomeKind::Failed,
            AgentRunStatus::WaitingApproval => SubAgentOutcomeKind::Failed,
        };
        Ok(SubAgentResult {
            child_run,
            kind,
            summary,
            details,
            run: Some(run_result),
            partial: false,
        })
    }
}

fn validate_delegation(request: &AgentDelegationRequest) -> AgentResult<()> {
    if request.parent_session_id.trim().is_empty()
        || request.parent_turn_id.trim().is_empty()
        || request.child_agent_id.trim().is_empty()
        || request.child_profile_id.trim().is_empty()
    {
        return Err(AgentError::invalid_input(
            "delegation requires parent session/turn and child agent/profile ids",
        ));
    }
    Ok(())
}

fn messages_from_input(input: &Value) -> Vec<AgentMessage> {
    if let Some(text) = input.as_str() {
        return vec![AgentMessage::user(text)];
    }
    if let Some(text) = input.get("text").and_then(Value::as_str) {
        return vec![AgentMessage::user(text)];
    }
    if let Some(messages) = input.get("messages").and_then(Value::as_array) {
        let parsed = messages
            .iter()
            .filter_map(|value| serde_json::from_value::<AgentMessage>(value.clone()).ok())
            .collect::<Vec<_>>();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    vec![AgentMessage::user(input.to_string())]
}

fn cancelled_result(request: &AgentDelegationRequest, summary: &str) -> SubAgentResult {
    SubAgentResult {
        child_run: ChildAgentRunRef {
            child_run_id: "cancelled".into(),
            parent_session_id: request.parent_session_id.clone(),
            child_session_id: String::new(),
            child_profile_id: request.child_profile_id.clone(),
            attempt_id: String::new(),
            depth: request.depth,
        },
        kind: SubAgentOutcomeKind::Cancelled,
        summary: summary.into(),
        details: None,
        run: None,
        partial: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::DelegationMode;

    fn orchestrator() -> SubAgentOrchestrator {
        let orch = SubAgentOrchestrator::new(Arc::new(EchoChildExecutor));
        orch.register_agent(SubAgentDescriptor {
            agent_id: "researcher".into(),
            profile_id: "profile.research".into(),
            summary: "research".into(),
            tools: vec!["search".into(), "read".into()],
            permissions: vec!["network".into()],
            max_depth: 3,
        })
        .unwrap();
        orch.register_agent(SubAgentDescriptor {
            agent_id: "writer".into(),
            profile_id: "profile.write".into(),
            summary: "write".into(),
            tools: vec!["write".into()],
            permissions: vec!["workspace".into()],
            max_depth: 3,
        })
        .unwrap();
        orch.bind_parent(
            "parent",
            DelegationScope {
                allowed_tools: vec!["search".into(), "read".into(), "write".into()],
                allowed_permissions: vec!["network".into(), "workspace".into()],
                inherit_conversation: true,
                inherit_knowledge: false,
            },
            DelegationBudget {
                max_total_tokens: Some(10_000),
                max_cost_microunits: Some(5_000),
                deadline_unix_ms: Some(9_999),
                max_child_steps: Some(4),
            },
        );
        orch
    }

    fn delegation(agent: &str, profile: &str, text: &str) -> AgentDelegationRequest {
        AgentDelegationRequest {
            parent_session_id: "parent".into(),
            parent_turn_id: "turn-1".into(),
            child_agent_id: agent.into(),
            child_profile_id: profile.into(),
            mode: DelegationMode::AgentAsTool,
            budget: DelegationBudget::default(),
            scope: DelegationScope {
                allowed_tools: vec!["search".into()],
                allowed_permissions: vec!["network".into()],
                inherit_conversation: true,
                inherit_knowledge: false,
            },
            input: Value::String(text.into()),
            input_ref: None,
            depth: 0,
        }
    }

    #[test]
    fn agent_as_tool_completes_two_level_chain() {
        let orch = orchestrator();
        let first = orch
            .invoke_as_tool(delegation("researcher", "profile.research", "alpha"))
            .unwrap();
        assert_eq!(first.kind, SubAgentOutcomeKind::Succeeded);
        assert!(first.summary.contains("alpha"));
        assert!(first.details.is_some());
        assert_eq!(first.child_run.depth, 1);
        assert_eq!(orch.children_of("parent").len(), 1);
    }

    #[test]
    fn handoff_tracks_chain_and_rejects_cycles() {
        let orch = orchestrator();
        orch.handoff(AgentHandoffRequest {
            session_id: "session-1".into(),
            from_profile_id: "profile.a".into(),
            to_profile_id: "profile.b".into(),
            inherit_conversation: true,
            inherit_tools: false,
            inherit_knowledge: false,
            inherit_budget: true,
            inherit_permissions: false,
            reason: "specialize".into(),
            chain: Vec::new(),
        })
        .unwrap();
        let err = orch
            .handoff(AgentHandoffRequest {
                session_id: "session-1".into(),
                from_profile_id: "profile.b".into(),
                to_profile_id: "profile.a".into(),
                inherit_conversation: true,
                inherit_tools: false,
                inherit_knowledge: false,
                inherit_budget: true,
                inherit_permissions: false,
                reason: "loop".into(),
                chain: vec!["profile.a".into(), "profile.b".into()],
            })
            .unwrap_err();
        assert_eq!(err.code, "agent.subagent.handoff_cycle");
        assert_eq!(orch.handoff_chain("session-1").len(), 1);
    }

    #[test]
    fn parallel_four_children_aggregate_stably() {
        let orch = orchestrator();
        let started = std::time::Instant::now();
        let result = orch
            .delegate_parallel(ParallelDelegationRequest {
                parent_session_id: "parent".into(),
                parent_turn_id: "turn-p".into(),
                children: (0..4)
                    .map(|index| {
                        let mut request =
                            delegation("researcher", "profile.research", &format!("item-{index}"));
                        request.parent_turn_id = format!("turn-{index}");
                        request
                    })
                    .collect(),
                policy: AggregationPolicy::All,
                budget: DelegationBudget {
                    max_total_tokens: Some(8_000),
                    ..DelegationBudget::default()
                },
            })
            .unwrap();
        let elapsed = started.elapsed();
        assert!(result.completed);
        assert_eq!(result.results.len(), 4);
        assert_eq!(orch.children_of("parent").len(), 4);
        assert!(
            elapsed.as_millis() < 200,
            "parallel aggregation smoke exceeded 200ms: {elapsed:?}"
        );
    }

    #[test]
    fn budget_scope_cancel_and_fencing_behave() {
        let orch = orchestrator();
        let mut denied = delegation("researcher", "profile.research", "secret");
        denied.scope.allowed_tools = vec!["shell".into()];
        assert_eq!(
            orch.invoke_as_tool(denied).unwrap_err().code,
            "agent.subagent.scope_denied"
        );

        orch.cancel_parent("parent");
        let cancelled = orch
            .invoke_as_tool(delegation("researcher", "profile.research", "late"))
            .unwrap();
        assert_eq!(cancelled.kind, SubAgentOutcomeKind::Cancelled);

        let orch = orchestrator();
        let first = orch
            .invoke_as_tool(delegation("researcher", "profile.research", "once"))
            .unwrap();
        assert_eq!(first.kind, SubAgentOutcomeKind::Succeeded);
        assert!(
            !orch
                .inner
                .completed_attempts
                .lock()
                .unwrap()
                .insert(first.child_run.attempt_id.clone()),
            "completed attempt must remain fenced"
        );
        assert!(
            !orch
                .inner
                .committed_side_effects
                .lock()
                .unwrap()
                .insert(format!("{}:side-effect", first.child_run.child_run_id)),
            "committed side effects must remain fenced"
        );
    }

    #[test]
    fn child_failure_does_not_break_parent_child_index() {
        struct FailExecutor;
        impl ChildAgentExecutor for FailExecutor {
            fn execute_child(&self, _request: AgentRunRequest) -> AgentResult<AgentRunResult> {
                Err(AgentError::new("child.boom", "child failed"))
            }
        }
        let orch = SubAgentOrchestrator::new(Arc::new(FailExecutor));
        orch.register_agent(SubAgentDescriptor {
            agent_id: "researcher".into(),
            profile_id: "profile.research".into(),
            summary: "research".into(),
            tools: vec!["search".into()],
            permissions: Vec::new(),
            max_depth: 2,
        })
        .unwrap();
        orch.bind_parent(
            "parent",
            DelegationScope {
                allowed_tools: vec!["search".into()],
                allowed_permissions: Vec::new(),
                inherit_conversation: false,
                inherit_knowledge: false,
            },
            DelegationBudget::default(),
        );
        let failed = orch
            .invoke_as_tool(delegation("researcher", "profile.research", "x"))
            .unwrap();
        assert_eq!(failed.kind, SubAgentOutcomeKind::Failed);
        assert_eq!(orch.children_of("parent").len(), 1);
        let ok_orch = orchestrator();
        let ok = ok_orch
            .invoke_as_tool(delegation("writer", "profile.write", "ok"))
            .unwrap();
        assert_eq!(ok.kind, SubAgentOutcomeKind::Succeeded);
    }
}
