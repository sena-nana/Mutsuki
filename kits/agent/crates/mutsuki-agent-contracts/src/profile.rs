use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentBudget, AgentKnowledgePolicy, AgentSkillPolicy, CredentialRef, ToolSideEffect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimeMode {
    Test,
    Development,
    #[default]
    Production,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRuntimePolicy {
    pub max_concurrent_turns: u32,
    pub max_parallel_context_providers: u32,
    pub max_parallel_tools: u32,
    pub cancellation_grace_ms: u64,
    pub streaming_enabled: bool,
}

impl Default for AgentRuntimePolicy {
    fn default() -> Self {
        Self {
            max_concurrent_turns: 32,
            max_parallel_context_providers: 16,
            max_parallel_tools: 16,
            cancellation_grace_ms: 5_000,
            streaming_enabled: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProviderInstance {
    pub instance_id: String,
    pub adapter_id: String,
    /// Opaque credential reference only. Secret material stays in Host secret boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<CredentialRef>,
    #[serde(default)]
    pub capability_tags: Vec<String>,
    #[serde(default)]
    pub endpoint_profile: Option<String>,
    #[serde(default)]
    pub test_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProtocolAdapterSelection {
    pub protocol_family: String,
    pub adapter_id: String,
    pub provider_instance_id: String,
    pub model: String,
    #[serde(default)]
    pub fallback_provider_instance_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfilePlugin {
    pub plugin_id: String,
    pub generation: u64,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPromptFragment {
    pub fragment_id: String,
    pub content: String,
    pub priority: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentContextPolicy {
    #[serde(default)]
    pub provider_ids: Vec<String>,
    #[serde(default)]
    pub budget: crate::ContextBudget,
    #[serde(default)]
    pub compaction_service: Option<String>,
    #[serde(default)]
    pub provider_options: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPermissionPolicy {
    #[serde(default)]
    pub auto_allow: Vec<ToolSideEffect>,
    #[serde(default)]
    pub require_approval: Vec<ToolSideEffect>,
    #[serde(default)]
    pub allowed_permissions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPersistenceDistributionPolicy {
    #[serde(default)]
    pub durable_sessions: bool,
    #[serde(default)]
    pub session_store_service: Option<String>,
    #[serde(default)]
    pub database_service: Option<String>,
    #[serde(default)]
    pub distributed_service: Option<String>,
    #[serde(default)]
    pub remote_adapters: bool,
    #[serde(default)]
    pub remote_subagents: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRuntimeProfile {
    pub profile_id: String,
    #[serde(default)]
    pub mode: AgentRuntimeMode,
    #[serde(default)]
    pub runtime: AgentRuntimePolicy,
    #[serde(default)]
    pub adapters: Vec<AgentProtocolAdapterSelection>,
    #[serde(default)]
    pub providers: Vec<AgentProviderInstance>,
    #[serde(default)]
    pub plugins: Vec<AgentProfilePlugin>,
    #[serde(default)]
    pub system_instructions: Vec<String>,
    #[serde(default)]
    pub prompt_fragments: Vec<AgentPromptFragment>,
    #[serde(default)]
    pub context: AgentContextPolicy,
    #[serde(default)]
    pub permissions: AgentPermissionPolicy,
    #[serde(default)]
    pub budget: AgentBudget,
    #[serde(default)]
    pub persistence_distribution: AgentPersistenceDistributionPolicy,
    #[serde(default)]
    pub skill: AgentSkillPolicy,
    #[serde(default)]
    pub knowledge: AgentKnowledgePolicy,
}
