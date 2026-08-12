use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentRunBudget, AgentRunRequest, AgentRunResult, ResourceRef};

/// Declares a callable child agent that Runtime may invoke as a tool or handoff target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubAgentDescriptor {
    pub agent_id: String,
    pub profile_id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub max_depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationMode {
    AgentAsTool,
    Handoff,
    Parallel,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_microunits: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_child_steps: Option<u32>,
}

impl From<AgentRunBudget> for DelegationBudget {
    fn from(value: AgentRunBudget) -> Self {
        Self {
            max_context_tokens: value.max_context_tokens,
            max_total_tokens: value.max_total_tokens,
            max_cost_microunits: value.max_cost_microunits,
            deadline_unix_ms: None,
            max_child_steps: None,
        }
    }
}

impl DelegationBudget {
    /// Intersect parent remaining budget with an explicit child allotment.
    pub fn intersect(&self, child: &Self) -> Self {
        Self {
            max_context_tokens: min_opt_u64(self.max_context_tokens, child.max_context_tokens),
            max_total_tokens: min_opt_u64(self.max_total_tokens, child.max_total_tokens),
            max_cost_microunits: min_opt_u64(self.max_cost_microunits, child.max_cost_microunits),
            deadline_unix_ms: min_opt_u64(self.deadline_unix_ms, child.deadline_unix_ms),
            max_child_steps: min_opt_u32(self.max_child_steps, child.max_child_steps),
        }
    }

    pub fn into_run_budget(self) -> AgentRunBudget {
        AgentRunBudget {
            max_context_tokens: self.max_context_tokens,
            max_total_tokens: self.max_total_tokens,
            max_cost_microunits: self.max_cost_microunits,
        }
    }
}

fn min_opt_u64(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn min_opt_u32(left: Option<u32>, right: Option<u32>) -> Option<u32> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationScope {
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_permissions: Vec<String>,
    #[serde(default)]
    pub inherit_conversation: bool,
    #[serde(default)]
    pub inherit_knowledge: bool,
}

impl DelegationScope {
    /// Child scope is the intersection of parent capability and requested scope.
    pub fn intersect(&self, requested: &Self) -> Self {
        Self {
            allowed_tools: intersect_lists(&self.allowed_tools, &requested.allowed_tools),
            allowed_permissions: intersect_lists(
                &self.allowed_permissions,
                &requested.allowed_permissions,
            ),
            inherit_conversation: self.inherit_conversation && requested.inherit_conversation,
            inherit_knowledge: self.inherit_knowledge && requested.inherit_knowledge,
        }
    }
}

fn intersect_lists(parent: &[String], requested: &[String]) -> Vec<String> {
    if requested.is_empty() {
        return parent.to_vec();
    }
    if parent.is_empty() {
        return Vec::new();
    }
    requested
        .iter()
        .filter(|item| parent.iter().any(|allowed| allowed == *item))
        .cloned()
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentDelegationRequest {
    pub parent_session_id: String,
    pub parent_turn_id: String,
    pub child_agent_id: String,
    pub child_profile_id: String,
    pub mode: DelegationMode,
    #[serde(default)]
    pub budget: DelegationBudget,
    #[serde(default)]
    pub scope: DelegationScope,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_ref: Option<ResourceRef>,
    #[serde(default)]
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentHandoffRequest {
    pub session_id: String,
    pub from_profile_id: String,
    pub to_profile_id: String,
    #[serde(default)]
    pub inherit_conversation: bool,
    #[serde(default)]
    pub inherit_tools: bool,
    #[serde(default)]
    pub inherit_knowledge: bool,
    #[serde(default)]
    pub inherit_budget: bool,
    #[serde(default)]
    pub inherit_permissions: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub chain: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildAgentRunRef {
    pub child_run_id: String,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub child_profile_id: String,
    pub attempt_id: String,
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentOutcomeKind {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    BudgetExceeded,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub child_run: ChildAgentRunRef,
    pub kind: SubAgentOutcomeKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<AgentRunResult>,
    #[serde(default)]
    pub partial: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregationPolicy {
    All,
    FirstSuccess,
    Quorum { required: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParallelDelegationRequest {
    pub parent_session_id: String,
    pub parent_turn_id: String,
    pub children: Vec<AgentDelegationRequest>,
    pub policy: AggregationPolicy,
    #[serde(default)]
    pub budget: DelegationBudget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParallelDelegationResult {
    pub policy: AggregationPolicy,
    pub results: Vec<SubAgentResult>,
    pub completed: bool,
    pub cancelled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentAsToolInvocation {
    pub tool_name: String,
    pub delegation: AgentDelegationRequest,
    pub child_run: AgentRunRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffRecord {
    pub from_profile_id: String,
    pub to_profile_id: String,
    pub at_unix_ms: u64,
    pub reason: String,
}
