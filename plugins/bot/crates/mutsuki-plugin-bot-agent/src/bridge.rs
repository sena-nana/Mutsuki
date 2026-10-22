#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Clone)]
pub struct BotAgentBridge {
    pub(crate) conversations: ConversationService,
    pub(crate) client: Arc<Mutex<Box<dyn AgentBridgeClient>>>,
    pub(crate) config: BotAgentConfigHandle,
    pub(crate) concurrency: Arc<BotAgentConcurrencyGate>,
}

#[derive(Default)]
pub(crate) struct BotAgentConcurrencyGate {
    pub(crate) active: Mutex<usize>,
}

pub(crate) struct BotAgentConcurrencyPermit {
    pub(crate) gate: Arc<BotAgentConcurrencyGate>,
}

impl BotAgentConcurrencyGate {
    pub(crate) fn try_acquire(self: &Arc<Self>, limit: usize) -> Option<BotAgentConcurrencyPermit> {
        let mut active = self.active.lock().expect("Bot Agent gate mutex");
        if *active >= limit.max(1) {
            return None;
        }
        *active += 1;
        Some(BotAgentConcurrencyPermit { gate: self.clone() })
    }
}

impl Drop for BotAgentConcurrencyPermit {
    fn drop(&mut self) {
        let mut active = self.gate.active.lock().expect("Bot Agent gate mutex");
        *active = active.saturating_sub(1);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotAgentTraceContext {
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
}

impl BotAgentBridge {
    #[must_use]
    /// Creates a bridge with a validated default policy and selected streaming strategy.
    ///
    /// # Panics
    ///
    /// Panics only if an internal enum-to-config mapping produces an invalid default snapshot.
    pub fn new(
        conversations: ConversationService,
        client: Box<dyn AgentBridgeClient>,
        streaming: &QqStreamingStrategy,
    ) -> Self {
        let config = BotAgentConfigHandle::default();
        let mut settings = config.snapshot();
        settings.enabled = true;
        settings.connection_id = "injected".into();
        settings.streaming = streaming_name(streaming).into();
        config
            .replace(settings)
            .expect("streaming strategy must produce a valid Bot Agent config");
        Self::new_with_config(conversations, client, config)
    }

    #[must_use]
    pub fn new_with_config(
        conversations: ConversationService,
        client: Box<dyn AgentBridgeClient>,
        config: BotAgentConfigHandle,
    ) -> Self {
        Self {
            conversations,
            client: Arc::new(Mutex::new(client)),
            config,
            concurrency: Arc::new(BotAgentConcurrencyGate::default()),
        }
    }

    /// Resolves, admits, and submits one Bot event to the bound Agent session.
    ///
    /// # Errors
    ///
    /// Returns a typed policy, session, media, or Agent execution error.
    pub async fn submit_event(
        &self,
        event: &BotEvent,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        self.submit_event_with_trace(event, None).await
    }

    /// Submits one Bot event while preserving the supplied trace context.
    ///
    /// # Errors
    ///
    /// Returns a typed policy, session, media, or Agent execution error.
    pub async fn submit_event_with_trace(
        &self,
        event: &BotEvent,
        trace: Option<&BotAgentTraceContext>,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        let result = self.submit_event_with_trace_deferred(event, trace).await?;
        self.complete_event(event).await?;
        Ok(result)
    }

    pub(crate) async fn submit_event_with_trace_deferred(
        &self,
        event: &BotEvent,
        trace: Option<&BotAgentTraceContext>,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let (resolved, config) = self.resolve_admitted_with_config(event).await?;
        let profile_id = resolved
            .policy
            .agent_runtime_profile_id
            .clone()
            .ok_or(BotAgentError::AgentProfileMissing)?;
        let message = event_message(event, trace)?;
        let turn_id = format!("qq:{}", event.event_id);
        let binding = self
            .conversations
            .get_or_create_session_binding(&resolved, actor_id)
            .await?;
        if self
            .conversations
            .begin_agent_event(&resolved, actor_id, &event.event_id, &turn_id)
            .await?
            == AgentEventClaim::Completed
        {
            return Ok(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: Vec::new(),
            });
        }
        let next_version = {
            let mut client = self
                .client
                .lock()
                .map_err(|_| BotAgentError::ClientPoisoned)?;
            match client.get_session(&binding.session_id) {
                Ok(_) => {}
                Err(error) if error.code == "agent.session.not_found" => {
                    client.start_session(AgentSessionCreateRequest {
                        session_id: Some(binding.session_id.clone()),
                        profile_id,
                        title: Some(format!("QQ {}", resolved.conversation.origin_key())),
                    })?;
                }
                Err(error) => return Err(error.into()),
            }
            client.submit_turn(
                &binding.session_id,
                SessionVersion(binding.session_version),
                &turn_id,
                vec![message],
                &event.event_id,
            )?
        };
        let binding = self
            .advance_or_reuse_session_version(&resolved, actor_id, &binding, next_version.0)
            .await?;
        let page = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .events(&binding.session_id, binding.last_event_sequence)?;
        let strategy = config.streaming_strategy()?;
        let outgoing =
            outgoing_messages_with_limit(event, &page, &strategy, config.max_message_bytes);
        let binding = self
            .conversations
            .advance_event_sequence(
                &resolved,
                actor_id,
                binding.last_event_sequence,
                page.next_sequence,
            )
            .await?;
        Ok(BotAgentBridgeResult {
            resolved,
            binding,
            turn_id,
            outgoing,
        })
    }

    pub(crate) async fn claim_event_state(
        &self,
        event: &BotEvent,
    ) -> Result<(AgentEventClaim, BotAgentBridgeResult), BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let (resolved, _) = self.resolve_admitted_with_config(event).await?;
        let binding = self
            .conversations
            .get_or_create_session_binding(&resolved, actor_id)
            .await?;
        let turn_id = format!("qq:{}", event.event_id);
        let claim = self
            .conversations
            .begin_agent_event(&resolved, actor_id, &event.event_id, &turn_id)
            .await?;
        Ok((
            claim,
            BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: Vec::new(),
            },
        ))
    }

    pub(crate) async fn complete_event(&self, event: &BotEvent) -> Result<(), BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        self.conversations
            .complete_agent_event(&resolved, actor_id, &event.event_id)
            .await?;
        Ok(())
    }

    /// Resolves the effective conversation policy for an event.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported targets, invalid identity, or repository failure.
    #[allow(clippy::unused_async_trait_impl)]
    pub async fn resolve(
        &self,
        event: &BotEvent,
    ) -> Result<ResolvedConversationPolicy, BotAgentError> {
        Ok(self
            .conversations
            .resolve_execution(qq_conversation_from_event(event)?)?)
    }

    pub(crate) async fn advance_or_reuse_session_version(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        binding: &AgentSessionBinding,
        next_session_version: u64,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        if next_session_version != binding.session_version {
            return Ok(self
                .conversations
                .advance_session(
                    resolved,
                    actor_id,
                    binding.session_version,
                    next_session_version,
                )
                .await?);
        }

        // AgentClient may return the already-accepted version when a reconnect retries the
        // same idempotency key after the first submit succeeded but event retrieval disconnected.
        // Reusing the persisted binding is safe only while reset/fork/concurrent progress has not
        // replaced this session lineage.
        let current = self
            .conversations
            .session_binding(resolved, actor_id)
            .await?
            .ok_or(ConversationError::BindingNotFound)?;
        if current.session_id != binding.session_id {
            return Err(ConversationError::GenerationConflict.into());
        }
        if current.session_version != next_session_version {
            return Err(ConversationError::SessionVersionConflict {
                expected: next_session_version,
                actual: current.session_version,
            }
            .into());
        }
        Ok(current)
    }

    /// Resolves policy and enforces admission without submitting an Agent turn.
    ///
    /// # Errors
    ///
    /// Returns the stable policy or admission error.
    pub async fn resolve_admitted(
        &self,
        event: &BotEvent,
    ) -> Result<ResolvedConversationPolicy, BotAgentError> {
        self.resolve_admitted_with_config(event)
            .await
            .map(|(resolved, _)| resolved)
    }

    pub(crate) async fn resolve_admitted_with_config(
        &self,
        event: &BotEvent,
    ) -> Result<(ResolvedConversationPolicy, BotAgentConfig), BotAgentError> {
        let config = self.config.snapshot();
        if !config.enabled {
            return Err(BotAgentError::AgentDisabled);
        }
        let mut resolved = self.resolve(event).await?;
        if let Some(profile_id) = event
            .ext
            .get(BOT_EXT_AGENT_PROFILE_ID)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            resolved.policy.agent_runtime_profile_id = Some(profile_id.to_owned());
        }
        if resolved.policy.agent_runtime_profile_id.is_none()
            && !config.default_profile_id.trim().is_empty()
        {
            resolved.policy.agent_runtime_profile_id = Some(config.default_profile_id.clone());
        }
        Ok((resolved, config))
    }

    /// Reads or creates the current Agent session binding for an admitted event.
    ///
    /// # Errors
    ///
    /// Returns an error when admission, scope derivation, or repository access fails.
    pub async fn status(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve(event).await?;
        self.conversations
            .session_binding(&resolved, actor_id)
            .await?
            .ok_or(BotAgentError::SessionBindingMissing)
    }

    /// Cancels a turn and atomically advances the conversation binding.
    ///
    /// # Errors
    ///
    /// Returns a typed Agent or binding version error.
    pub async fn cancel(
        &self,
        resolved: &ResolvedConversationPolicy,
        binding: &AgentSessionBinding,
        actor_id: Option<&str>,
        turn_id: &str,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        let version = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .cancel_turn(
                &binding.session_id,
                turn_id,
                SessionVersion(binding.session_version),
            )?;
        Ok(self
            .conversations
            .advance_session(resolved, actor_id, binding.session_version, version.0)
            .await?)
    }

    /// Resolves an event and cancels the selected Agent turn.
    ///
    /// # Errors
    ///
    /// Returns a typed admission, Agent, or binding error.
    pub async fn cancel_event(
        &self,
        event: &BotEvent,
        turn_id: &str,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        let binding = self.status(event).await?;
        self.cancel(&resolved, &binding, actor_id, turn_id).await
    }

    /// Replaces the event's current session binding with a fresh session.
    ///
    /// # Errors
    ///
    /// Returns a typed admission or repository error.
    pub async fn reset(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        Ok(self
            .conversations
            .reset_session_binding(&resolved, actor_id)
            .await?)
    }

    /// Forks the event's session and commits the new binding atomically.
    ///
    /// # Errors
    ///
    /// Returns a typed admission, Agent, or binding conflict error.
    pub async fn fork(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        let (source, target_session_id) = self
            .conversations
            .prepare_session_fork(&resolved, actor_id)
            .await?;
        let target_version = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .fork_session(
                &source.session_id,
                &target_session_id,
                SessionVersion(source.session_version),
            )?;
        Ok(self
            .conversations
            .commit_session_fork(
                &resolved,
                actor_id,
                &source,
                target_session_id,
                target_version.0,
            )
            .await?)
    }

    /// Resubmits an event under a new idempotency identity for regeneration.
    ///
    /// # Errors
    ///
    /// Returns a typed status, admission, or Agent execution error.
    pub async fn regenerate(
        &self,
        event: &BotEvent,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        let binding = self.status(event).await?;
        let mut retry = event.clone();
        retry.event_id = format!("{}:regenerate:{}", event.event_id, binding.generation);
        self.submit_event(&retry).await
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BotAgentBridgeResult {
    pub resolved: ResolvedConversationPolicy,
    pub binding: AgentSessionBinding,
    pub turn_id: String,
    pub outgoing: Vec<BotMessage>,
}

pub(crate) fn event_message(
    event: &BotEvent,
    trace: Option<&BotAgentTraceContext>,
) -> Result<AgentMessage, BotAgentError> {
    let message = event
        .message
        .as_ref()
        .ok_or(BotAgentError::MessageMissing)?;
    let mut parts = Vec::new();
    for segment in &message.segments {
        match segment {
            MessageSegment::Image { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Image {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    width: None,
                    height: None,
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::Audio { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Audio {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    duration_ms: None,
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::File { resource, name } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Document {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    filename: name.clone(),
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::Video { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Document {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    filename: Some("qq-video".into()),
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            _ => {}
        }
    }
    let mut agent_message = AgentMessage::user(message.plain_text()).with_parts(parts);
    let mut metadata = serde_json::json!({
        "source": "qq",
        "account_id": &event.bot.account_id,
        "event_id": &event.event_id,
        "message_id": message.message_id.as_deref(),
        "reply_to": message.reply_to.as_deref(),
        "trace_id": trace.and_then(|trace| trace.trace_id.as_deref()),
        "correlation_id": trace.and_then(|trace| trace.correlation_id.as_deref()),
    });
    let mut injections = Vec::new();
    if let Some(icl) = event.ext.get(BOT_EXT_CONVERSATION_ICL) {
        metadata["icl"] = icl.clone();
        if let Ok(entries) = serde_json::from_value::<Vec<ConversationIclEntry>>(icl.clone()) {
            let summary = format_icl_summary(&entries);
            if !summary.is_empty() {
                injections.push(AgentMessageContextInjection {
                    source_kind: CONTEXT_SOURCE_ICL.into(),
                    text: summary,
                    source_id: resolved_origin_key(event),
                });
            }
        }
    }
    if let Some(identifiers) = event.ext.get(BOT_EXT_CONVERSATION_IDENTIFIERS) {
        metadata["identifiers"] = identifiers.clone();
        if let Ok(identifiers) =
            <ConversationIdentifiers as serde::Deserialize>::deserialize(identifiers)
            && let Some(text) = identifiers.prompt_text()
        {
            injections.push(AgentMessageContextInjection {
                source_kind: CONTEXT_SOURCE_IDENTIFIERS.into(),
                text,
                source_id: resolved_origin_key(event),
            });
        }
    }
    if let Some(prompt) = event
        .ext
        .get(BOT_EXT_PERSONA_PROMPT)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        metadata["persona_prompt"] = serde_json::Value::String(prompt.to_owned());
        injections.push(AgentMessageContextInjection {
            source_kind: CONTEXT_SOURCE_PERSONA.into(),
            text: prompt.to_owned(),
            source_id: "persona".into(),
        });
    }
    if !injections.is_empty() {
        metadata[AGENT_MESSAGE_CONTEXT_INJECTIONS_META] = serde_json::to_value(&injections)
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
    }
    agent_message.metadata = Some(metadata);
    Ok(agent_message)
}

pub(crate) fn resolved_origin_key(event: &BotEvent) -> String {
    qq_conversation_from_event(event)
        .map(|conversation| conversation.origin_key())
        .unwrap_or_else(|_| event.event_id.clone())
}

pub(crate) fn validate_agent_media(
    resource: &mutsuki_runtime_contracts::ResourceRef,
) -> Result<(), BotAgentError> {
    if resource.provider_id.trim().is_empty()
        || resource.schema.trim().is_empty()
        || resource.size_hint.is_none()
        || resource.content_hash.as_deref().is_none_or(str::is_empty)
    {
        return Err(BotAgentError::MediaResourceUnvalidated);
    }
    Ok(())
}

pub(crate) fn outgoing_messages_with_limit(
    source: &BotEvent,
    page: &AgentEventPage,
    strategy: &QqStreamingStrategy,
    max_message_bytes: usize,
) -> Vec<BotMessage> {
    let mut deltas = String::new();
    let mut final_text = None;
    let mut supplements = String::new();
    let mut artifacts = Vec::new();
    for envelope in &page.events {
        match &envelope.event {
            AgentEvent::ModelDelta { text, .. } => deltas.push_str(text),
            AgentEvent::FinalResponse { summary, .. } => final_text = Some(summary.clone()),
            AgentEvent::ArtifactProduced { artifact, .. } => {
                if let Some(resource) = artifact.content_ref.clone() {
                    if resource.size_hint.is_some()
                        && resource
                            .content_hash
                            .as_deref()
                            .is_some_and(|hash| !hash.is_empty())
                    {
                        let segment = if artifact.media_type.starts_with("image/") {
                            MessageSegment::Image { resource }
                        } else if artifact.media_type.starts_with("audio/") {
                            MessageSegment::Audio { resource }
                        } else if artifact.media_type.starts_with("video/") {
                            MessageSegment::Video { resource }
                        } else {
                            MessageSegment::File {
                                resource,
                                name: Some(artifact.artifact_id.clone()),
                            }
                        };
                        artifacts.push(BotMessage {
                            message_id: None,
                            target: source.target.clone(),
                            sender: None,
                            segments: vec![segment],
                            reply_to: None,
                            time_ms: None,
                            ext: std::collections::BTreeMap::default(),
                        });
                    } else {
                        let _ = write!(
                            supplements,
                            "\n\n[{}: unavailable resource]",
                            artifact.summary
                        );
                    }
                } else if !artifact.summary.is_empty() {
                    let _ = write!(supplements, "\n\n{}", artifact.summary);
                }
            }
            _ => {}
        }
    }
    let mut text = match strategy {
        QqStreamingStrategy::FinalOnly => final_text.unwrap_or(deltas),
        QqStreamingStrategy::SegmentMessages => {
            if deltas.is_empty() {
                final_text.unwrap_or_default()
            } else {
                deltas
            }
        }
    };
    text.push_str(&supplements);
    if text.is_empty() {
        return artifacts;
    }
    // QQ delivery has one size boundary regardless of whether the bridge emits deltas or a
    // final response. Final-only controls event selection, not an unlimited transport payload.
    let mut messages = split_text(&text, max_message_bytes)
        .into_iter()
        .map(|chunk| BotMessage::text(source.target.clone(), chunk))
        .collect::<Vec<_>>();
    messages.extend(artifacts);
    messages
}

pub(crate) fn trace_context(task: &Task) -> Option<BotAgentTraceContext> {
    (task.trace_id.is_some() || task.correlation_id.is_some()).then(|| BotAgentTraceContext {
        trace_id: task.trace_id.clone().map(Into::into),
        correlation_id: task.correlation_id.clone(),
    })
}

pub(crate) fn split_text(text: &str, max_bytes: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    if max_bytes < BOT_AGENT_MIN_MESSAGE_BYTES {
        return vec![text.to_owned()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(text[start..end].to_owned());
        start = end;
    }
    chunks
}

pub(crate) fn streaming_name(strategy: &QqStreamingStrategy) -> &'static str {
    match strategy {
        QqStreamingStrategy::FinalOnly => "final_only",
        QqStreamingStrategy::SegmentMessages => "segment_messages",
    }
}
