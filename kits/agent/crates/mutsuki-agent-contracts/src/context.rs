use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentMemoryRecord, AgentMessage, AgentToolDescriptor, ResourceRef, SessionVersion};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudget {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub max_items: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPriority {
    Required,
    High,
    Normal,
    Low,
}

impl ContextPriority {
    pub const fn rank(&self) -> u8 {
        match self {
            Self::Required => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Low => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProvenance {
    pub provider_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub source_version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextItemRef {
    pub item_id: String,
    pub category: String,
    pub summary: String,
    #[serde(default)]
    pub details: Option<ResourceRef>,
    pub priority: ContextPriority,
    pub estimated_tokens: u64,
    pub estimated_bytes: u64,
    pub provenance: ContextProvenance,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextProviderSpec {
    pub provider_id: String,
    pub priority: ContextPriority,
    pub required: bool,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextProviderBatchRequest {
    pub session_id: String,
    pub turn_id: String,
    pub session_version: SessionVersion,
    pub providers: Vec<ContextProviderSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDecisionKind {
    Included,
    DroppedBudget,
    DroppedOptionalProviderFailure,
    CompactionRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDecision {
    pub item_id: String,
    pub decision: ContextDecisionKind,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextPlan {
    pub session_id: String,
    pub turn_id: String,
    pub session_version: SessionVersion,
    pub budget: ContextBudget,
    pub items: Vec<ContextItemRef>,
    pub decisions: Vec<ContextDecision>,
    pub total_tokens: u64,
    pub total_bytes: u64,
    pub stable_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactionRequest {
    pub session_id: String,
    pub turn_id: String,
    pub source_snapshot: ResourceRef,
    pub target_budget: ContextBudget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactionResult {
    pub compacted_snapshot: ResourceRef,
    pub summary: String,
    pub provenance: ContextProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentContextCompactionConfig {
    pub service_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentContextBuildRequest {
    pub profile_id: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub max_context_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<AgentContextCompactionConfig>,
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
    #[serde(default)]
    pub preparation_usage: crate::AgentUsage,
    #[serde(default)]
    pub preparation_cost_microunits: u64,
}
