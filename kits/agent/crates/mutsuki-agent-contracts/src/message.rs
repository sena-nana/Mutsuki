use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AgentContentPart;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: AgentRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Multimodal parts. Large media must use ResourceRef inside parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<AgentContentPart>,
}

impl AgentMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: AgentRole::User,
            content: content.into(),
            name: None,
            metadata: None,
            parts: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: AgentRole::Assistant,
            content: content.into(),
            name: None,
            metadata: None,
            parts: Vec::new(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: AgentRole::System,
            content: content.into(),
            name: None,
            metadata: None,
            parts: Vec::new(),
        }
    }

    pub fn with_parts(mut self, parts: Vec<AgentContentPart>) -> Self {
        self.parts = parts;
        self
    }
}
