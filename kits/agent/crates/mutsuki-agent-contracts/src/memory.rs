use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ResourceCellRef, ResourceRef};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryScopeRef {
    /// Neutral namespace (tenant/product/session). Must not encode Lilia project types.
    pub namespace: String,
    #[serde(default)]
    pub scope_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryProvenance {
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRevision {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub updated_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub memory_id: String,
    pub text: String,
    pub scope: MemoryScopeRef,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub score: f32,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_unix_ms: Option<u64>,
    #[serde(default)]
    pub provenance: MemoryProvenance,
    #[serde(default)]
    pub revision: MemoryRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Large body must live behind ResourceRef.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<ResourceCellRef>,
}

/// Backward-compatible query/write surface used by existing MemoryRouter protocols.
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

impl From<&MemoryEntry> for AgentMemoryRecord {
    fn from(entry: &MemoryEntry) -> Self {
        Self {
            memory_id: entry.memory_id.clone(),
            text: entry.text.clone(),
            tags: entry.tags.clone(),
            score: entry.score,
            metadata: entry.metadata.clone(),
            resource: entry.resource.clone(),
            cell: entry.cell.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMemoryQueryRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MemoryScopeRef>,
    #[serde(default)]
    pub include_disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_unix_ms: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MemoryScopeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<MemoryProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details_ref: Option<ResourceRef>,
}

impl AgentMemoryWriteRequest {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tags: Vec::new(),
            metadata: None,
            scope: None,
            priority: None,
            confidence: None,
            expiry_unix_ms: None,
            provenance: None,
            details_ref: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMemoryActivateRequest {
    pub memory_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMemoryDisableRequest {
    pub memory_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMemoryDeleteRequest {
    pub memory_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentMemoryReviseRequest {
    pub memory_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details_ref: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub query: String,
    pub scope: MemoryScopeRef,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryCandidate {
    pub entry: MemoryEntry,
    pub rank: u32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOpportunityMode {
    HintOnly,
    Ask,
    AutoInject,
    AgentDecides,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryOpportunity {
    pub opportunity_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub mode: MemoryOpportunityMode,
    pub candidates: Vec<MemoryCandidate>,
    pub trigger: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAdoptionDecision {
    Adopted,
    Rejected,
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryAdoptionFeedback {
    pub opportunity_id: String,
    pub memory_id: String,
    pub decision: MemoryAdoptionDecision,
    #[serde(default)]
    pub reason: String,
    pub recorded_at_unix_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBaselinePolicy {
    #[default]
    Disabled,
    ScopeSelected,
    ProfileSelected {
        provider_id: String,
    },
}

fn default_limit() -> usize {
    8
}
