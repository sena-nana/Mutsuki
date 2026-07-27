use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ResourceCellRef, ResourceRef};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentMemoryRecord {
    pub memory_id: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Provider-backed memory value handle. Not Core StateStore private state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceRef>,
    /// Provider cell that owns the memory slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<ResourceCellRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMemoryQueryRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentMemoryQueryResult {
    pub records: Vec<AgentMemoryRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentMemoryWriteRequest {
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMemoryActivateRequest {
    pub memory_id: String,
}

fn default_limit() -> usize {
    8
}
