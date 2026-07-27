use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSideEffect {
    #[default]
    None,
    WorkspaceRead,
    WorkspaceWrite,
    ExternalRead,
    ExternalWrite,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolDescriptor {
    pub name: String,
    pub target_protocol_id: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Value,
    #[serde(default)]
    pub side_effect: ToolSideEffect,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub permissions: Vec<String>,
}

impl AgentToolDescriptor {
    pub fn new(
        name: impl Into<String>,
        target_protocol_id: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            target_protocol_id: target_protocol_id.into(),
            description: description.into(),
            input_schema: json!({}),
            output_schema: json!({}),
            side_effect: ToolSideEffect::None,
            requires_approval: false,
            permissions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentToolListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentToolListResult {
    pub tools: Vec<AgentToolDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolExecuteRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolExecuteResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<String>,
    #[serde(default)]
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub input: Value,
}
