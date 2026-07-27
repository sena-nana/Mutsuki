use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentRunBudget, AgentToolCall, PermissionRequest, ResourceRef};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionVersion(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionStatus {
    Active,
    WaitingApproval,
    Completed,
    Cancelled,
    Failed,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTurnStatus {
    Created,
    CollectingContext,
    Generating,
    RunningTools,
    WaitingApproval,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStepStatus {
    Pending,
    Submitted,
    Waiting,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    MaxSteps,
    TokenBudget,
    CostBudget,
    Deadline,
    Cancelled,
    ApprovalRejected,
    ApprovalTimedOut,
    ModelFailure,
    ToolFailure,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBudget {
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub max_total_tokens: Option<u64>,
    #[serde(default)]
    pub max_cost_microunits: Option<u64>,
    #[serde(default)]
    pub deadline_unix_ms: Option<u64>,
}

impl From<AgentRunBudget> for AgentBudget {
    fn from(value: AgentRunBudget) -> Self {
        Self {
            max_steps: None,
            max_total_tokens: value.max_total_tokens,
            max_cost_microunits: value.max_cost_microunits,
            deadline_unix_ms: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AgentAttemptOutcome {
    Succeeded,
    Cancelled,
    DeadlineExceeded,
    RetryableFailure { code: String },
    NonRetryableFailure { code: String },
    BudgetExceeded { reason: StopReason },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingModelCall {
    pub attempt_id: String,
    pub task_id: String,
    pub step_index: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingToolCall {
    pub attempt_id: String,
    pub task_id: String,
    pub step_index: u32,
    pub call: AgentToolCall,
    pub side_effect_committed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingApproval {
    pub request: PermissionRequest,
    pub tool_call: AgentToolCall,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStepState {
    pub step_index: u32,
    pub status: AgentStepStatus,
    #[serde(default)]
    pub pending_model: Option<PendingModelCall>,
    #[serde(default)]
    pub pending_tools: Vec<PendingToolCall>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentTurnState {
    pub turn_id: String,
    pub status: AgentTurnStatus,
    pub expected_version: SessionVersion,
    #[serde(default)]
    pub steps: Vec<AgentStepState>,
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionState {
    pub session_id: String,
    pub profile_id: String,
    pub version: SessionVersion,
    pub status: AgentSessionStatus,
    pub budget: AgentBudget,
    pub snapshot: ResourceRef,
    #[serde(default)]
    pub turns: Vec<AgentTurnState>,
    #[serde(default)]
    pub pending_approvals: Vec<PendingApproval>,
    #[serde(default)]
    pub completed_attempts: BTreeSet<String>,
    #[serde(default)]
    pub committed_side_effects: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelGenerateTask {
    pub session_id: String,
    pub turn_id: String,
    pub step_index: u32,
    pub attempt_id: String,
    pub expected_version: SessionVersion,
    pub request: crate::AgentModelGenerateRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolInvokeTask {
    pub session_id: String,
    pub turn_id: String,
    pub step_index: u32,
    pub attempt_id: String,
    pub expected_version: SessionVersion,
    pub call: AgentToolCall,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextCollectTask {
    pub session_id: String,
    pub turn_id: String,
    pub provider_id: String,
    pub expected_version: SessionVersion,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextCompactTask {
    pub session_id: String,
    pub turn_id: String,
    pub source: ResourceRef,
    pub expected_version: SessionVersion,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubAgentTask {
    pub parent_session_id: String,
    pub parent_turn_id: String,
    pub child_profile_id: String,
    pub budget: AgentRunBudget,
    #[serde(default)]
    pub input: Value,
}
use std::collections::BTreeSet;
