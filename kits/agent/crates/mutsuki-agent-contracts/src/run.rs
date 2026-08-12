use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentEventEnvelope, AgentMessage, AgentUsage, InteractionRequest, InteractionResolution,
    PermissionDecision, PermissionRequest, ResourceRef,
};

pub const DEFAULT_MAX_STEPS: u32 = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunBudget {
    /// Maximum tokens admitted into one model request after context assembly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_microunits: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Completed,
    WaitingApproval,
    WaitingInteraction,
    BudgetExceeded,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPermissionMode {
    #[default]
    Ask,
    Full,
    ReadOnly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRunRequest {
    pub profile_id: String,
    /// New messages for this invocation. When `session_id` is present the
    /// persisted session history is loaded first, so approval-only resume
    /// requests leave this empty instead of replaying prior messages.
    pub messages: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    #[serde(default)]
    pub budget: AgentRunBudget,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub permission_mode: AgentPermissionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_protocol_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_context: Option<Value>,
    #[serde(default)]
    pub permission_decisions: Vec<PermissionDecision>,
    #[serde(default)]
    pub interaction_resolutions: Vec<InteractionResolution>,
}

impl AgentRunRequest {
    pub fn new(profile_id: impl Into<String>, messages: Vec<AgentMessage>) -> Self {
        Self {
            profile_id: profile_id.into(),
            messages,
            session_id: None,
            turn_id: None,
            max_steps: DEFAULT_MAX_STEPS,
            budget: AgentRunBudget::default(),
            stream: false,
            permission_mode: AgentPermissionMode::Ask,
            model: None,
            provider_hint: None,
            metadata: None,
            result_protocol_id: None,
            result_context: None,
            permission_decisions: Vec::new(),
            interaction_resolutions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStepRecord {
    pub step_index: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub status: AgentRunStatus,
    pub messages: Vec<AgentMessage>,
    #[serde(default)]
    pub steps: Vec<AgentStepRecord>,
    #[serde(default)]
    pub usage: AgentUsage,
    #[serde(default)]
    pub cost_microunits: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_resource: Option<ResourceRef>,
    #[serde(default)]
    pub pending_approvals: Vec<PermissionRequest>,
    #[serde(default)]
    pub pending_interactions: Vec<InteractionRequest>,
    /// Monotonic session events produced by this invocation. Durable sessions
    /// persist these alongside the transcript for reconnect/replay.
    #[serde(default)]
    pub events: Vec<AgentEventEnvelope>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRunResultCallback {
    pub result: AgentRunResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

fn default_max_steps() -> u32 {
    DEFAULT_MAX_STEPS
}
