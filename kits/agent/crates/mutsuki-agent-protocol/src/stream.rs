use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ResourceRef;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentStreamEvent {
    MessageDelta { text: String },
    ToolCall { name: String, input: Value },
    ToolResult { name: String, output: Value },
    Done,
}

/// Open stream handle backed by a Core `ResourceRef`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStreamHandle {
    pub stream: ResourceRef,
}
