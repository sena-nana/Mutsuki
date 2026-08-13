//! Context assembly owned by the Runtime domain.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentContext, AgentContextBuildRequest, AgentContextCompactionConfig, AgentError,
    AgentMemoryRecord, AgentMessage, AgentResult, AgentToolDescriptor, AgentUsage,
    CompactionResult, ContextBudget, ContextItemRef, ContextPriority, ContextProvenance,
    ResourceRef, SessionVersion,
};
use sha2::{Digest, Sha256};

use crate::{
    AgentResourceStore, CompactionCancellation, CompactionDisposition,
    ContextCompactionCoordinator, ContextCompactionPreparation, PendingContextCompaction,
    TranscriptCompactionCandidate, TranscriptContextWindow, estimate_messages_tokens,
};

const TRANSCRIPT_SNAPSHOT_KIND: &str = "agent.context.transcript";
const TRANSCRIPT_SNAPSHOT_SCHEMA: &str = "agent.context.transcript@1";
const COMPACTED_TRANSCRIPT_KIND: &str = "agent.context.compacted-transcript";
const COMPACTED_TRANSCRIPT_SCHEMA: &str = "agent.context.compacted-transcript@1";
const MAX_COMPACTION_CACHE_ENTRIES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CompactionCacheKey {
    session_id: String,
    turn_id: String,
    service_id: String,
    model: String,
    provider_hint: Option<String>,
    summary_token_budget: u64,
    source_hash: String,
}

#[derive(Clone)]
enum CachedTranscriptCompaction {
    Summary(Box<CachedTranscriptSummary>),
    DeterministicFallback,
}

#[derive(Clone)]
struct CachedTranscriptSummary {
    content: String,
    provider_id: String,
    source_snapshot: ResourceRef,
}

#[derive(Clone, Debug)]
struct AgentContextParts {
    profile_id: String,
    tools: Vec<AgentToolDescriptor>,
    memories: Vec<AgentMemoryRecord>,
    rendered_prompt: Option<String>,
    preparation_usage: AgentUsage,
    preparation_cost_microunits: u64,
}

pub enum AgentContextBuildPreparation {
    Ready(AgentContext),
    Compaction(Box<PendingAgentContextCompaction>),
}

pub struct PendingAgentContextCompaction {
    parts: AgentContextParts,
    config: AgentContextCompactionConfig,
    transcript: TranscriptCompactionCandidate,
    source_snapshot: ResourceRef,
    pending: PendingContextCompaction,
    cache_key: CompactionCacheKey,
}

impl PendingAgentContextCompaction {
    pub fn config(&self) -> &AgentContextCompactionConfig {
        &self.config
    }

    pub fn source_messages(&self) -> &[AgentMessage] {
        &self.transcript.dropped_messages
    }

    pub fn source_snapshot(&self) -> &ResourceRef {
        &self.source_snapshot
    }

    pub fn max_output_tokens(&self) -> u64 {
        self.transcript.summary_token_budget
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentContextModelSummary {
    pub content: String,
    pub provider_id: String,
    pub usage: AgentUsage,
    pub cost_microunits: u64,
}

#[derive(Clone, Default)]
pub struct ContextBuilder {
    tools: Arc<Mutex<Vec<AgentToolDescriptor>>>,
    memories: Arc<Mutex<Vec<AgentMemoryRecord>>>,
    system_prompt: Arc<Mutex<Option<String>>>,
    transcript_window: TranscriptContextWindow,
    resources: AgentResourceStore,
    compaction: ContextCompactionCoordinator,
    compaction_cache: Arc<Mutex<BTreeMap<CompactionCacheKey, CachedTranscriptCompaction>>>,
}

impl ContextBuilder {
    pub fn with_resources(resources: AgentResourceStore) -> Self {
        Self {
            resources,
            ..Self::default()
        }
    }

    pub fn set_tools(&self, tools: Vec<AgentToolDescriptor>) {
        *self.tools.lock().expect("context builder mutex poisoned") = tools;
    }

    pub fn set_memories(&self, memories: Vec<AgentMemoryRecord>) {
        *self
            .memories
            .lock()
            .expect("context builder mutex poisoned") = memories;
    }

    pub fn set_system_prompt(&self, prompt: impl Into<String>) {
        *self
            .system_prompt
            .lock()
            .expect("context builder mutex poisoned") = Some(prompt.into());
    }

    pub fn build(&self, request: AgentContextBuildRequest) -> AgentResult<AgentContext> {
        let parts = self.context_parts(&request)?;
        Ok(self.deterministic_context(parts, &request))
    }

    pub fn prepare(
        &self,
        request: AgentContextBuildRequest,
    ) -> AgentResult<AgentContextBuildPreparation> {
        let parts = self.context_parts(&request)?;
        let Some(config) = request.compaction.clone() else {
            return Ok(AgentContextBuildPreparation::Ready(
                self.deterministic_context(parts, &request),
            ));
        };
        if config.service_id.trim().is_empty() || config.model.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "context compaction service_id and model are required",
            ));
        }
        let Some(limit) = request.max_context_tokens.filter(|limit| *limit > 0) else {
            return Ok(AgentContextBuildPreparation::Ready(
                self.deterministic_context(parts, &request),
            ));
        };
        let Some(transcript) = self
            .transcript_window
            .compaction_candidate(&request.messages, limit)
        else {
            return Ok(AgentContextBuildPreparation::Ready(
                self.deterministic_context(parts, &request),
            ));
        };
        if transcript.summary_token_budget == 0 {
            return Ok(AgentContextBuildPreparation::Ready(
                self.deterministic_context(parts, &request),
            ));
        }

        let source_bytes = serde_json::to_vec(&transcript.dropped_messages)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let session_id = request.session_id.as_deref().unwrap_or("ephemeral");
        let turn_id = request.turn_id.as_deref().unwrap_or("turn:context");
        let cache_key = CompactionCacheKey {
            session_id: session_id.to_owned(),
            turn_id: turn_id.to_owned(),
            service_id: config.service_id.clone(),
            model: config.model.clone(),
            provider_hint: config.provider_hint.clone(),
            summary_token_budget: transcript.summary_token_budget,
            source_hash: hex::encode(Sha256::digest(&source_bytes)),
        };
        let cached = self
            .compaction_cache
            .lock()
            .map_err(|_| {
                AgentError::new(
                    "agent.runtime.state_poisoned",
                    "context compaction cache lock poisoned",
                )
            })?
            .get(&cache_key)
            .cloned();
        if let Some(cached) = cached {
            return Ok(AgentContextBuildPreparation::Ready(match cached {
                CachedTranscriptCompaction::Summary(summary) => self.semantic_context(
                    parts,
                    config,
                    transcript,
                    &summary.source_snapshot,
                    summary.content,
                    summary.provider_id,
                ),
                CachedTranscriptCompaction::DeterministicFallback => {
                    self.deterministic_context(parts, &request)
                }
            }));
        }
        let source_value = serde_json::from_slice(&source_bytes)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let source_snapshot = self.resources.put_json(
            "mutsuki.agent.context",
            TRANSCRIPT_SNAPSHOT_KIND,
            TRANSCRIPT_SNAPSHOT_SCHEMA,
            1,
            &source_value,
        )?;
        let estimated_tokens = estimate_messages_tokens(&transcript.dropped_messages);
        let estimated_bytes = source_bytes.len() as u64;
        let item = ContextItemRef {
            item_id: format!("transcript:{}", source_snapshot.ref_id),
            category: "session".into(),
            summary: format!(
                "{} earlier transcript messages require semantic compaction",
                transcript.dropped_message_count
            ),
            details: Some(source_snapshot.clone()),
            priority: ContextPriority::Required,
            estimated_tokens,
            estimated_bytes,
            provenance: ContextProvenance {
                provider_id: "mutsuki.agent.context".into(),
                source_kind: "transcript".into(),
                source_id: source_snapshot.ref_id.clone(),
                source_version: request.messages.len().to_string(),
            },
            required: true,
        };
        let budget = ContextBudget {
            max_tokens: Some(transcript.summary_token_budget),
            max_bytes: Some(transcript.summary_token_budget.saturating_mul(4)),
            max_items: Some(1),
        };
        let version = SessionVersion(request.messages.len() as u64);
        let pending = match self.compaction.begin(
            session_id,
            turn_id,
            version,
            budget,
            vec![item],
            source_snapshot.clone(),
        )? {
            ContextCompactionPreparation::Ready(_) => {
                return Ok(AgentContextBuildPreparation::Ready(
                    self.deterministic_context(parts, &request),
                ));
            }
            ContextCompactionPreparation::Required(pending) => *pending,
        };
        Ok(AgentContextBuildPreparation::Compaction(Box::new(
            PendingAgentContextCompaction {
                parts,
                config,
                transcript,
                source_snapshot,
                pending,
                cache_key,
            },
        )))
    }

    pub fn complete_compaction(
        &self,
        pending: PendingAgentContextCompaction,
        outcome: Result<AgentContextModelSummary, AgentError>,
    ) -> AgentResult<AgentContext> {
        let PendingAgentContextCompaction {
            mut parts,
            config,
            transcript,
            source_snapshot,
            pending,
            cache_key,
        } = pending;
        if let Ok(summary) = &outcome {
            parts.preparation_usage = summary.usage.clone();
            parts.preparation_cost_microunits = summary.cost_microunits;
        }
        let cancellation = CompactionCancellation::default();
        let compacted = outcome.and_then(|summary| {
            let content = summary.content.trim().to_owned();
            if content.is_empty() {
                return Err(AgentError::new(
                    "agent.context.compaction_empty",
                    "context compaction provider returned an empty summary",
                ));
            }
            let compacted_snapshot = self.resources.put_json(
                "mutsuki.agent.context",
                COMPACTED_TRANSCRIPT_KIND,
                COMPACTED_TRANSCRIPT_SCHEMA,
                1,
                &serde_json::json!({
                    "summary": content,
                    "source": source_snapshot,
                    "serviceId": config.service_id,
                    "model": config.model,
                    "providerId": summary.provider_id,
                }),
            )?;
            Ok(CompactionResult {
                compacted_snapshot,
                summary: content,
                provenance: ContextProvenance {
                    provider_id: summary.provider_id,
                    source_kind: "model_summary".into(),
                    source_id: source_snapshot.ref_id.clone(),
                    source_version: transcript.dropped_message_count.to_string(),
                },
            })
        });
        let prepared = self.compaction.finish(pending, compacted, &cancellation)?;
        if prepared.compaction != CompactionDisposition::Completed {
            self.cache_compaction(cache_key, CachedTranscriptCompaction::DeterministicFallback)?;
            return Ok(self.context_from_messages(
                parts,
                self.transcript_window
                    .prepare(
                        &joined_transcript(&transcript),
                        Some(transcript.max_context_tokens),
                    )
                    .messages,
            ));
        }
        let Some(item) = prepared.plan.items.first() else {
            return Err(AgentError::new(
                "agent.context.compaction_invalid",
                "completed context compaction did not produce a summary item",
            ));
        };
        let content = item.summary.clone();
        let provider_id = item.provenance.provider_id.clone();
        self.cache_compaction(
            cache_key,
            CachedTranscriptCompaction::Summary(Box::new(CachedTranscriptSummary {
                content: content.clone(),
                provider_id: provider_id.clone(),
                source_snapshot: source_snapshot.clone(),
            })),
        )?;
        Ok(self.semantic_context(
            parts,
            config,
            transcript,
            &source_snapshot,
            content,
            provider_id,
        ))
    }

    fn context_parts(&self, request: &AgentContextBuildRequest) -> AgentResult<AgentContextParts> {
        if request.profile_id.trim().is_empty() {
            return Err(AgentError::invalid_input("profile_id is required"));
        }
        let tools = self
            .tools
            .lock()
            .expect("context builder mutex poisoned")
            .clone();
        let memories = self
            .memories
            .lock()
            .expect("context builder mutex poisoned")
            .clone();
        let rendered_prompt = self
            .system_prompt
            .lock()
            .expect("context builder mutex poisoned")
            .clone();
        Ok(AgentContextParts {
            profile_id: request.profile_id.clone(),
            tools,
            memories,
            rendered_prompt,
            preparation_usage: AgentUsage::default(),
            preparation_cost_microunits: 0,
        })
    }

    fn deterministic_context(
        &self,
        parts: AgentContextParts,
        request: &AgentContextBuildRequest,
    ) -> AgentContext {
        let prepared = self
            .transcript_window
            .prepare(&request.messages, request.max_context_tokens);
        self.context_from_messages(parts, prepared.messages)
    }

    fn context_from_messages(
        &self,
        parts: AgentContextParts,
        messages: Vec<AgentMessage>,
    ) -> AgentContext {
        AgentContext {
            profile_id: parts.profile_id,
            messages,
            tools: parts.tools,
            memories: parts.memories,
            rendered_prompt: parts.rendered_prompt,
            preparation_usage: parts.preparation_usage,
            preparation_cost_microunits: parts.preparation_cost_microunits,
        }
    }

    fn semantic_context(
        &self,
        parts: AgentContextParts,
        config: AgentContextCompactionConfig,
        transcript: TranscriptCompactionCandidate,
        source_snapshot: &ResourceRef,
        content: String,
        provider_id: String,
    ) -> AgentContext {
        let mut summary = AgentMessage::system(content);
        summary.metadata = Some(serde_json::json!({
            "context_compaction": {
                "strategy": "model_summary_v1",
                "service_id": config.service_id,
                "model": config.model,
                "provider_id": provider_id,
                "dropped_messages": transcript.dropped_message_count,
                "source_snapshot": source_snapshot.ref_id,
            }
        }));
        let mut messages = transcript.system_messages;
        messages.push(summary);
        messages.extend(transcript.retained_messages);
        trim_compaction_summary(&mut messages, transcript.max_context_tokens);
        self.context_from_messages(parts, messages)
    }

    fn cache_compaction(
        &self,
        key: CompactionCacheKey,
        value: CachedTranscriptCompaction,
    ) -> AgentResult<()> {
        let mut cache = self.compaction_cache.lock().map_err(|_| {
            AgentError::new(
                "agent.runtime.state_poisoned",
                "context compaction cache lock poisoned",
            )
        })?;
        if cache.len() >= MAX_COMPACTION_CACHE_ENTRIES
            && let Some(oldest) = cache.keys().next().cloned()
        {
            cache.remove(&oldest);
        }
        cache.insert(key, value);
        Ok(())
    }
}

fn joined_transcript(candidate: &TranscriptCompactionCandidate) -> Vec<AgentMessage> {
    let mut messages = candidate.system_messages.clone();
    messages.extend(candidate.dropped_messages.clone());
    messages.extend(candidate.retained_messages.clone());
    messages
}

fn trim_compaction_summary(messages: &mut [AgentMessage], max_context_tokens: u64) {
    let Some(index) = messages.iter().position(|message| {
        message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("context_compaction"))
            .is_some()
    }) else {
        return;
    };
    while estimate_messages_tokens(messages) > max_context_tokens {
        let content = &mut messages[index].content;
        let Some((offset, _)) = content.char_indices().next_back() else {
            break;
        };
        content.truncate(offset);
    }
}

#[cfg(test)]
mod tests {
    use mutsuki_agent_contracts::{AgentContextCompactionConfig, AgentRole};

    use super::*;

    fn request() -> AgentContextBuildRequest {
        AgentContextBuildRequest {
            profile_id: "coding".into(),
            messages: vec![
                AgentMessage::system("system"),
                AgentMessage::user(format!("old request {}", "context ".repeat(500))),
                AgentMessage::assistant(format!("old result {}", "result ".repeat(500))),
                AgentMessage::user("latest request"),
            ],
            session_id: Some("session-1".into()),
            turn_id: Some("turn-2".into()),
            max_context_tokens: Some(300),
            compaction: Some(AgentContextCompactionConfig {
                service_id: "mutsuki.agent.context.compaction@1".into(),
                model: "summary-model".into(),
                provider_hint: Some("provider-a".into()),
            }),
            metadata: None,
        }
    }

    #[test]
    fn semantic_compaction_replaces_only_the_model_context_history() {
        let builder = ContextBuilder::default();
        let request = request();
        let AgentContextBuildPreparation::Compaction(pending) =
            builder.prepare(request.clone()).unwrap()
        else {
            panic!("oversized transcript should request semantic compaction");
        };
        assert!(
            pending
                .source_messages()
                .iter()
                .any(|message| message.content.starts_with("old request"))
        );

        let context = builder
            .complete_compaction(
                *pending,
                Ok(AgentContextModelSummary {
                    content: "User asked for the old change; the implementation completed it."
                        .into(),
                    provider_id: "provider-a".into(),
                    usage: AgentUsage {
                        input_tokens: 120,
                        output_tokens: 16,
                        total_tokens: 136,
                    },
                    cost_microunits: 42,
                }),
            )
            .unwrap();

        assert!(context.messages.iter().any(|message| {
            message.role == AgentRole::System
                && message.content.contains("old change")
                && message.metadata.as_ref().is_some_and(|metadata| {
                    metadata
                        .pointer("/context_compaction/strategy")
                        .and_then(serde_json::Value::as_str)
                        == Some("model_summary_v1")
                })
        }));
        assert!(
            context
                .messages
                .iter()
                .any(|message| message.content == "latest request")
        );
        assert!(
            !context
                .messages
                .iter()
                .any(|message| { message.content.starts_with("old request context") })
        );
        assert!(estimate_messages_tokens(&context.messages) <= 300);
        assert_eq!(context.preparation_usage.total_tokens, 136);
        assert_eq!(context.preparation_cost_microunits, 42);

        let AgentContextBuildPreparation::Ready(cached) = builder.prepare(request).unwrap() else {
            panic!("the same turn and transcript should reuse semantic compaction");
        };
        assert!(cached.messages.iter().any(|message| {
            message.content.contains("old change")
                && message.metadata.as_ref().is_some_and(|metadata| {
                    metadata
                        .pointer("/context_compaction/strategy")
                        .and_then(serde_json::Value::as_str)
                        == Some("model_summary_v1")
                })
        }));
        assert_eq!(cached.preparation_usage, AgentUsage::default());
        assert_eq!(cached.preparation_cost_microunits, 0);
    }

    #[test]
    fn semantic_compaction_failure_uses_the_deterministic_history_digest() {
        let builder = ContextBuilder::default();
        let request = request();
        let AgentContextBuildPreparation::Compaction(pending) =
            builder.prepare(request.clone()).unwrap()
        else {
            panic!("oversized transcript should request semantic compaction");
        };
        let context = builder
            .complete_compaction(*pending, Err(AgentError::provider_unavailable("offline")))
            .unwrap();

        assert!(context.messages.iter().any(|message| {
            message.metadata.as_ref().is_some_and(|metadata| {
                metadata
                    .pointer("/context_compaction/strategy")
                    .and_then(serde_json::Value::as_str)
                    == Some("deterministic_turn_window_v1")
            })
        }));
        assert!(
            context
                .messages
                .iter()
                .any(|message| message.content == "latest request")
        );
        assert!(matches!(
            builder.prepare(request).unwrap(),
            AgentContextBuildPreparation::Ready(_)
        ));
    }
}
