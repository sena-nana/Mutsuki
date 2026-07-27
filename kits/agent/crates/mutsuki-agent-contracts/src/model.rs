use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentMessage, AgentToolCall, AgentUsage, ResourceRef};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentModelStopReason {
    #[default]
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentModelGenerateRequest {
    pub model: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_protocol_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentModelGenerateResult {
    pub message: AgentMessage,
    #[serde(default)]
    pub stop_reason: AgentModelStopReason,
    #[serde(default)]
    pub tool_calls: Vec<AgentToolCall>,
    #[serde(default)]
    pub usage: AgentUsage,
    #[serde(default)]
    pub cost_microunits: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_resource: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentModelStreamRequest {
    pub request: AgentModelGenerateRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentModelStreamResult {
    pub stream: ResourceRef,
    #[serde(default)]
    pub stop_reason: AgentModelStopReason,
    #[serde(default)]
    pub tool_calls: Vec<AgentToolCall>,
    #[serde(default)]
    pub usage: AgentUsage,
    #[serde(default)]
    pub cost_microunits: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentModelResultCallback {
    pub result: AgentModelGenerateResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "request", rename_all = "snake_case")]
pub enum AgentModelHttpEffectRequest {
    Generate(AgentModelGenerateRequest),
    Stream(AgentModelStreamRequest),
}
