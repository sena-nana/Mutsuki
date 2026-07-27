use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentMemoryRecord, AgentMessage, AgentToolDescriptor};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentContextBuildRequest {
    pub profile_id: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub max_context_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentContext {
    pub profile_id: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default)]
    pub tools: Vec<AgentToolDescriptor>,
    #[serde(default)]
    pub memories: Vec<AgentMemoryRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_prompt: Option<String>,
}
