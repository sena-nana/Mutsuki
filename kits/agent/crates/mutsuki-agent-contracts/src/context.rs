use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentMemoryRecord, AgentMessage, AgentRole, AgentToolDescriptor, ResourceRef, SessionVersion,
};

pub const CONTEXT_SOURCE_ICL: &str = "icl";
pub const CONTEXT_SOURCE_IDENTIFIERS: &str = "identifiers";
pub const CONTEXT_SOURCE_PERSONA: &str = "persona";
pub const CONTEXT_SOURCE_MEMORY: &str = "memory";
pub const CONTEXT_SOURCE_SKILL: &str = "skill";
pub const CONTEXT_SOURCE_KNOWLEDGE: &str = "knowledge";
pub const CONTEXT_SOURCE_PROMPT: &str = "prompt";
pub const CONTEXT_SOURCE_PROVIDER: &str = "context_provider";
pub const AGENT_MESSAGE_CONTEXT_INJECTIONS_META: &str = "context_injections";

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

impl ContextProvenance {
    pub fn new(
        provider_id: impl Into<String>,
        source_kind: impl Into<String>,
        source_id: impl Into<String>,
        source_version: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            source_kind: source_kind.into(),
            source_id: source_id.into(),
            source_version: source_version.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextInjection {
    pub text: String,
    pub provenance: ContextProvenance,
}

impl ContextInjection {
    pub fn new(text: impl Into<String>, provenance: ContextProvenance) -> Self {
        Self {
            text: text.into(),
            provenance,
        }
    }
}

/// Model-visible injection declared on a user message instead of stuffed into
/// the user text. Agent context records each entry as `ContextInjected`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageContextInjection {
    pub source_kind: String,
    pub text: String,
    #[serde(default)]
    pub source_id: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_fragments: Vec<crate::AgentPromptFragment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_template_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_query: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ContextProviderSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<crate::RetrievalQuery>,
    #[serde(default)]
    pub discover_skills: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_version: Option<SessionVersion>,
}

impl AgentContextBuildRequest {
    pub fn new(profile_id: impl Into<String>, messages: Vec<AgentMessage>) -> Self {
        Self {
            profile_id: profile_id.into(),
            messages,
            session_id: None,
            turn_id: None,
            max_context_tokens: None,
            compaction: None,
            metadata: None,
            system_instructions: Vec::new(),
            prompt_fragments: Vec::new(),
            prompt_template_id: None,
            memory_query: None,
            providers: Vec::new(),
            knowledge: None,
            discover_skills: false,
            session_version: None,
        }
    }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injections: Vec<ContextInjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_plan: Option<ContextPlan>,
    #[serde(default)]
    pub preparation_usage: crate::AgentUsage,
    #[serde(default)]
    pub preparation_cost_microunits: u64,
}

impl AgentContext {
    /// Projects the model-visible message list: recorded injections first, then
    /// the prepared transcript. Injections already present as system messages
    /// are not duplicated.
    pub fn model_messages(&self) -> Vec<AgentMessage> {
        apply_injections_to_messages(&self.injections, &self.messages)
    }
}

/// Prepends injection texts as system messages so the model request matches the
/// session log projection.
pub fn apply_injections_to_messages(
    injections: &[ContextInjection],
    transcript: &[AgentMessage],
) -> Vec<AgentMessage> {
    let mut messages = transcript.to_vec();
    for injection in injections.iter().rev() {
        if injection.text.trim().is_empty() {
            continue;
        }
        if messages
            .iter()
            .any(|message| message.role == AgentRole::System && message.content == injection.text)
        {
            continue;
        }
        messages.insert(0, AgentMessage::system(injection.text.clone()));
    }
    messages
}

pub fn injections_from_user_metadata(
    messages: &[AgentMessage],
    provider_id: &str,
    source_version: &str,
) -> Vec<ContextInjection> {
    let Some(metadata) = messages
        .iter()
        .rev()
        .find(|message| message.role == AgentRole::User)
        .and_then(|message| message.metadata.as_ref())
    else {
        return Vec::new();
    };
    let Some(entries) = metadata
        .get(AGENT_MESSAGE_CONTEXT_INJECTIONS_META)
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<AgentMessageContextInjection>>(value).ok())
    else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter(|entry| !entry.text.trim().is_empty())
        .map(|entry| {
            let source_id = if entry.source_id.trim().is_empty() {
                entry.source_kind.clone()
            } else {
                entry.source_id
            };
            ContextInjection::new(
                entry.text,
                ContextProvenance::new(provider_id, entry.source_kind, source_id, source_version),
            )
        })
        .collect()
}

pub fn injections_from_context_plan(plan: &ContextPlan) -> Vec<ContextInjection> {
    plan.items
        .iter()
        .filter(|item| !item.summary.trim().is_empty())
        .map(|item| ContextInjection::new(item.summary.clone(), item.provenance.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_metadata_and_plan_become_injections_and_model_messages() {
        let mut user = AgentMessage::user("hello");
        user.metadata = Some(json!({
            AGENT_MESSAGE_CONTEXT_INJECTIONS_META: [
                {"source_kind": CONTEXT_SOURCE_ICL, "text": "群聊：alice: hi", "source_id": "group"},
                {"source_kind": CONTEXT_SOURCE_PERSONA, "text": "you are a helper"}
            ]
        }));
        let from_meta = injections_from_user_metadata(&[user.clone()], "bot.conversation", "3");
        assert_eq!(from_meta.len(), 2);
        assert_eq!(from_meta[0].provenance.source_kind, CONTEXT_SOURCE_ICL);
        assert_eq!(from_meta[1].provenance.source_id, CONTEXT_SOURCE_PERSONA);

        let plan = ContextPlan {
            session_id: "s".into(),
            turn_id: "t".into(),
            session_version: SessionVersion(3),
            budget: ContextBudget::default(),
            items: vec![ContextItemRef {
                item_id: "provider:git".into(),
                category: "provider".into(),
                summary: "main @ abc (1 change(s))".into(),
                details: None,
                priority: ContextPriority::Normal,
                estimated_tokens: 8,
                estimated_bytes: 32,
                provenance: ContextProvenance::new(
                    "mutsuki.agent.context.git",
                    CONTEXT_SOURCE_PROVIDER,
                    "mutsuki.agent.context.git",
                    "3",
                ),
                required: false,
            }],
            decisions: Vec::new(),
            total_tokens: 8,
            total_bytes: 32,
            stable_hash: "hash".into(),
        };
        let mut injections = from_meta;
        injections.extend(injections_from_context_plan(&plan));
        let context = AgentContext {
            profile_id: "p".into(),
            messages: vec![user],
            tools: Vec::new(),
            memories: Vec::new(),
            rendered_prompt: None,
            injections,
            context_plan: Some(plan),
            preparation_usage: crate::AgentUsage::default(),
            preparation_cost_microunits: 0,
        };
        let model = context.model_messages();
        assert_eq!(model[0].role, AgentRole::System);
        assert!(model.iter().any(|message| message.content.contains("群聊")));
        assert!(
            model
                .iter()
                .any(|message| message.content.contains("main @ abc"))
        );
        assert_eq!(
            model.last().map(|message| message.content.as_str()),
            Some("hello")
        );
    }
}
