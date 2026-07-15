use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentMessage, AgentUsage, ResourceRef};

pub const DEFAULT_MAX_STEPS: u32 = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunBudget {
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
    BudgetExceeded,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRunRequest {
    pub profile_id: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    #[serde(default)]
    pub budget: AgentRunBudget,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_protocol_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_context: Option<Value>,
}

impl AgentRunRequest {
    pub fn new(profile_id: impl Into<String>, messages: Vec<AgentMessage>) -> Self {
        Self {
            profile_id: profile_id.into(),
            messages,
            session_id: None,
            max_steps: DEFAULT_MAX_STEPS,
            budget: AgentRunBudget::default(),
            stream: false,
            model: None,
            metadata: None,
            result_protocol_id: None,
            result_context: None,
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
