use std::sync::Arc;

use async_trait::async_trait;
use mutsuki_bot_protocol::{
    AgentSessionBinding, AgentSessionScope, BotConversationKind, BotEvent, BotTarget,
    ConversationPolicy, QQ_CONVERSATION_REF_VERSION, QqConversationRef, ResolvedConversationPolicy,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[async_trait]
pub trait ConversationRepository: Send + Sync {
    async fn session_binding(
        &self,
        binding_key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError>;

    async fn compare_and_set_session_binding(
        &self,
        binding_key: &str,
        expected_generation: Option<u64>,
        binding: AgentSessionBinding,
    ) -> Result<(), ConversationError>;

    async fn begin_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
        turn_id: &str,
    ) -> Result<AgentEventClaim, ConversationError>;

    async fn complete_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
    ) -> Result<(), ConversationError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentEventClaim {
    New,
    ResumePending,
    Completed,
}

#[derive(Clone)]
pub struct ConversationService {
    repository: Arc<dyn ConversationRepository>,
    product_default: ConversationPolicy,
}

impl ConversationService {
    pub fn new(
        repository: Arc<dyn ConversationRepository>,
        product_default: ConversationPolicy,
    ) -> Self {
        Self {
            repository,
            product_default,
        }
    }

    /// Resolves the Bot-owned execution settings without reading legacy trigger rules.
    /// Matching, permission and wake-up decisions are owned by the published Flow graph.
    pub fn resolve_execution(
        &self,
        conversation: QqConversationRef,
    ) -> Result<ResolvedConversationPolicy, ConversationError> {
        conversation
            .validate()
            .map_err(|error| ConversationError::InvalidConversationRef(error.to_string()))?;
        Ok(ResolvedConversationPolicy {
            conversation,
            policy: self.product_default.clone(),
        })
    }

    /// Loads or atomically creates the session binding for the resolved scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the scope key is invalid, storage fails, or CAS cannot converge.
    pub async fn get_or_create_session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<AgentSessionBinding, ConversationError> {
        let binding_key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        if let Some(binding) = self.repository.session_binding(&binding_key).await? {
            if binding.policy_revision == resolved.policy.revision {
                return Ok(binding);
            }
            let refreshed = AgentSessionBinding {
                policy_revision: resolved.policy.revision,
                generation: binding.generation.saturating_add(1),
                ..binding.clone()
            };
            return match self
                .repository
                .compare_and_set_session_binding(
                    &binding_key,
                    Some(binding.generation),
                    refreshed.clone(),
                )
                .await
            {
                Ok(()) => Ok(refreshed),
                Err(ConversationError::GenerationConflict) => self
                    .repository
                    .session_binding(&binding_key)
                    .await?
                    .ok_or(ConversationError::GenerationConflict),
                Err(error) => Err(error),
            };
        }
        let binding = AgentSessionBinding {
            origin_key: resolved.conversation.origin_key(),
            session_id: stable_session_id(&binding_key),
            session_version: 0,
            last_event_sequence: 0,
            policy_revision: resolved.policy.revision,
            generation: 1,
        };
        match self
            .repository
            .compare_and_set_session_binding(&binding_key, None, binding.clone())
            .await
        {
            Ok(()) => Ok(binding),
            Err(ConversationError::GenerationConflict) => self
                .repository
                .session_binding(&binding_key)
                .await?
                .ok_or(ConversationError::GenerationConflict),
            Err(error) => Err(error),
        }
    }

    /// Advances the bound Agent session with an optimistic version check.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid versions, missing bindings, storage failures, or CAS conflicts.
    pub async fn advance_session(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        expected_session_version: u64,
        next_session_version: u64,
    ) -> Result<AgentSessionBinding, ConversationError> {
        if next_session_version <= expected_session_version {
            return Err(ConversationError::InvalidSessionVersion);
        }
        let binding_key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        let current = self
            .repository
            .session_binding(&binding_key)
            .await?
            .ok_or(ConversationError::BindingNotFound)?;
        if current.session_version != expected_session_version {
            return Err(ConversationError::SessionVersionConflict {
                expected: expected_session_version,
                actual: current.session_version,
            });
        }
        let next = AgentSessionBinding {
            session_version: next_session_version,
            generation: current.generation + 1,
            policy_revision: resolved.policy.revision,
            ..current.clone()
        };
        self.repository
            .compare_and_set_session_binding(&binding_key, Some(current.generation), next.clone())
            .await?;
        Ok(next)
    }

    /// Advances the committed event sequence with an optimistic version check.
    ///
    /// # Errors
    ///
    /// Returns an error for missing bindings, non-monotonic sequences, or storage conflicts.
    pub async fn advance_event_sequence(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        expected_sequence: u64,
        next_sequence: u64,
    ) -> Result<AgentSessionBinding, ConversationError> {
        let binding_key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        let current = self
            .repository
            .session_binding(&binding_key)
            .await?
            .ok_or(ConversationError::BindingNotFound)?;
        if current.last_event_sequence != expected_sequence || next_sequence < expected_sequence {
            return Err(ConversationError::EventSequenceConflict {
                expected: expected_sequence,
                actual: current.last_event_sequence,
            });
        }
        let next = AgentSessionBinding {
            last_event_sequence: next_sequence,
            generation: current.generation + 1,
            ..current.clone()
        };
        self.repository
            .compare_and_set_session_binding(&binding_key, Some(current.generation), next.clone())
            .await?;
        Ok(next)
    }

    /// Reads the session binding for the resolved conversation scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the scope key is invalid or repository access fails.
    pub async fn session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        let key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        self.repository.session_binding(&key).await
    }

    /// Replaces the current binding with a fresh reset session.
    ///
    /// # Errors
    ///
    /// Returns an error when the binding is missing or the atomic replacement fails.
    pub async fn reset_session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<AgentSessionBinding, ConversationError> {
        self.replace_session_binding(resolved, actor_id, "reset")
            .await
    }

    /// Replaces the current binding after session expiry.
    ///
    /// # Errors
    ///
    /// Returns an error when the binding is missing or the atomic replacement fails.
    pub async fn expire_session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<AgentSessionBinding, ConversationError> {
        self.replace_session_binding(resolved, actor_id, "expire")
            .await
    }

    async fn replace_session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        reason: &str,
    ) -> Result<AgentSessionBinding, ConversationError> {
        let key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        let current = self
            .repository
            .session_binding(&key)
            .await?
            .ok_or(ConversationError::BindingNotFound)?;
        let generation = current.generation.saturating_add(1);
        let next = AgentSessionBinding {
            origin_key: current.origin_key,
            session_id: stable_session_id(&format!("{key}|{reason}:{generation}")),
            session_version: 0,
            last_event_sequence: 0,
            policy_revision: resolved.policy.revision,
            generation,
        };
        self.repository
            .compare_and_set_session_binding(&key, Some(current.generation), next.clone())
            .await?;
        Ok(next)
    }

    /// Computes a deterministic fork target without publishing it.
    ///
    /// # Errors
    ///
    /// Returns an error when the binding scope is invalid, missing, or unavailable.
    pub async fn prepare_session_fork(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<(AgentSessionBinding, String), ConversationError> {
        let key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        let current = self
            .repository
            .session_binding(&key)
            .await?
            .ok_or(ConversationError::BindingNotFound)?;
        let next_generation = current.generation.saturating_add(1);
        let target_session_id = stable_session_id(&format!("{key}|fork:{next_generation}"));
        Ok((current, target_session_id))
    }

    /// Atomically publishes a previously prepared session fork.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid target state, a stale source binding, or repository failure.
    pub async fn commit_session_fork(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        source: &AgentSessionBinding,
        target_session_id: String,
        target_session_version: u64,
    ) -> Result<AgentSessionBinding, ConversationError> {
        if target_session_id.trim().is_empty() || target_session_version == 0 {
            return Err(ConversationError::InvalidSessionVersion);
        }
        let key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        let current = self
            .repository
            .session_binding(&key)
            .await?
            .ok_or(ConversationError::BindingNotFound)?;
        if current.generation != source.generation || current.session_id != source.session_id {
            return Err(ConversationError::GenerationConflict);
        }
        let next = AgentSessionBinding {
            origin_key: current.origin_key,
            session_id: target_session_id,
            session_version: target_session_version,
            last_event_sequence: current.last_event_sequence,
            policy_revision: resolved.policy.revision,
            generation: current.generation.saturating_add(1),
        };
        self.repository
            .compare_and_set_session_binding(&key, Some(current.generation), next.clone())
            .await?;
        Ok(next)
    }

    /// Claims an Agent event exactly once for the bound conversation.
    ///
    /// # Errors
    ///
    /// Returns an error when scope derivation or the repository claim fails.
    pub async fn begin_agent_event(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        event_id: &str,
        turn_id: &str,
    ) -> Result<AgentEventClaim, ConversationError> {
        let key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        self.repository
            .begin_agent_event(&key, event_id, turn_id)
            .await
    }

    /// Marks a previously claimed Agent event as completed.
    ///
    /// # Errors
    ///
    /// Returns an error when scope derivation or repository completion fails.
    pub async fn complete_agent_event(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        event_id: &str,
    ) -> Result<(), ConversationError> {
        let key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        self.repository.complete_agent_event(&key, event_id).await
    }
}

/// Converts a QQ Bot event target into the canonical conversation identity.
///
/// # Errors
///
/// Returns an error for unsupported targets or invalid QQ conversation fields.
pub fn qq_conversation_from_event(
    event: &BotEvent,
) -> Result<QqConversationRef, ConversationError> {
    let (kind, user_id, group_id, guild_id, channel_id) = match &event.target {
        BotTarget::User { user_id } => (
            BotConversationKind::Private,
            Some(user_id.clone()),
            None,
            None,
            None,
        ),
        BotTarget::Group { group_id } => (
            BotConversationKind::Group,
            None,
            Some(group_id.clone()),
            None,
            None,
        ),
        BotTarget::GuildChannel {
            guild_id,
            channel_id,
        } => (
            BotConversationKind::Channel,
            None,
            None,
            Some(guild_id.clone()),
            Some(channel_id.clone()),
        ),
        BotTarget::Conversation { .. } | BotTarget::PlatformSpecific { .. } => {
            return Err(ConversationError::UnsupportedTarget);
        }
    };
    let conversation = QqConversationRef {
        version: QQ_CONVERSATION_REF_VERSION,
        account_id: event.bot.account_id.clone(),
        kind,
        user_id,
        group_id,
        guild_id,
        channel_id,
        thread_id: event
            .ext
            .get("qqbot.thread_id")
            .or_else(|| event.ext.get("qqbot.topic_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    };
    conversation
        .validate()
        .map_err(|error| ConversationError::InvalidConversationRef(error.to_string()))?;
    Ok(conversation)
}

/// Builds the stable repository key for a scoped Agent session binding.
///
/// # Errors
///
/// Returns an error for an invalid conversation or a missing actor in actor-scoped mode.
pub fn session_binding_key(
    conversation: &QqConversationRef,
    scope: AgentSessionScope,
    actor_id: Option<&str>,
) -> Result<String, ConversationError> {
    conversation
        .validate()
        .map_err(|error| ConversationError::InvalidConversationRef(error.to_string()))?;
    let origin = conversation.origin_key();
    match scope {
        AgentSessionScope::SharedConversation => Ok(origin),
        AgentSessionScope::ActorInConversation => actor_id
            .filter(|actor| !actor.trim().is_empty())
            .map(|actor| format!("{origin}|actor:{}:{actor}", actor.len()))
            .ok_or(ConversationError::ActorRequired),
    }
}

fn stable_session_id(binding_key: &str) -> String {
    let digest = Sha256::digest(binding_key.as_bytes());
    format!("bot-{}", hex::encode(&digest[..16]))
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ConversationError {
    #[error("conversation repository failed: {0}")]
    Repository(String),
    #[error("session binding generation conflict")]
    GenerationConflict,
    #[error("session binding was not found")]
    BindingNotFound,
    #[error("actor id is required by actor-in-conversation session policy")]
    ActorRequired,
    #[error("next Agent session version must advance")]
    InvalidSessionVersion,
    #[error("Agent session version conflict: expected {expected}, actual {actual}")]
    SessionVersionConflict { expected: u64, actual: u64 },
    #[error("Agent event sequence conflict: expected {expected}, actual {actual}")]
    EventSequenceConflict { expected: u64, actual: u64 },
    #[error("Bot event target cannot be represented as a QQ conversation")]
    UnsupportedTarget,
    #[error("invalid QQ conversation ref: {0}")]
    InvalidConversationRef(String),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use mutsuki_bot_protocol::{BotConversationKind, QQ_CONVERSATION_REF_VERSION};

    use super::*;

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        futures::executor::block_on(future)
    }

    #[derive(Default)]
    struct MemoryRepository {
        bindings: Mutex<BTreeMap<String, AgentSessionBinding>>,
        events: Mutex<BTreeMap<(String, String), bool>>,
    }

    #[async_trait]
    impl ConversationRepository for MemoryRepository {
        async fn session_binding(
            &self,
            binding_key: &str,
        ) -> Result<Option<AgentSessionBinding>, ConversationError> {
            Ok(self.bindings.lock().unwrap().get(binding_key).cloned())
        }

        async fn compare_and_set_session_binding(
            &self,
            binding_key: &str,
            expected_generation: Option<u64>,
            binding: AgentSessionBinding,
        ) -> Result<(), ConversationError> {
            let mut bindings = self.bindings.lock().unwrap();
            let actual = bindings.get(binding_key).map(|binding| binding.generation);
            if actual != expected_generation {
                return Err(ConversationError::GenerationConflict);
            }
            bindings.insert(binding_key.into(), binding);
            Ok(())
        }

        async fn begin_agent_event(
            &self,
            binding_key: &str,
            event_id: &str,
            _turn_id: &str,
        ) -> Result<AgentEventClaim, ConversationError> {
            let mut events = self.events.lock().unwrap();
            let key = (binding_key.into(), event_id.into());
            Ok(match events.get(&key) {
                Some(true) => AgentEventClaim::Completed,
                Some(false) => AgentEventClaim::ResumePending,
                None => {
                    events.insert(key, false);
                    AgentEventClaim::New
                }
            })
        }

        async fn complete_agent_event(
            &self,
            binding_key: &str,
            event_id: &str,
        ) -> Result<(), ConversationError> {
            self.events
                .lock()
                .unwrap()
                .insert((binding_key.into(), event_id.into()), true);
            Ok(())
        }
    }

    #[test]
    fn conversation_refs_round_trip_private_group_and_channel_targets() {
        let refs = [
            QqConversationRef {
                version: QQ_CONVERSATION_REF_VERSION,
                account_id: "main".into(),
                kind: BotConversationKind::Private,
                user_id: Some("user".into()),
                group_id: None,
                guild_id: None,
                channel_id: None,
                thread_id: Some("topic".into()),
            },
            group_conversation(),
            QqConversationRef {
                version: QQ_CONVERSATION_REF_VERSION,
                account_id: "main".into(),
                kind: BotConversationKind::Channel,
                user_id: None,
                group_id: None,
                guild_id: Some("guild".into()),
                channel_id: Some("channel".into()),
                thread_id: Some("thread".into()),
            },
        ];

        for conversation in refs {
            let key = conversation.origin_key();
            assert_eq!(
                QqConversationRef::from_origin_key(&key).unwrap(),
                conversation
            );
            assert!(conversation.target().is_some());
            let encoded = serde_json::to_string(&conversation).unwrap();
            assert_eq!(
                serde_json::from_str::<QqConversationRef>(&encoded).unwrap(),
                conversation
            );
        }
    }

    #[test]
    fn shared_and_actor_scopes_are_stable_and_version_fenced() {
        let repository = Arc::new(MemoryRepository::default());
        let shared = ConversationService::new(repository.clone(), default_policy());
        let conversation = group_conversation();
        let resolved = shared.resolve_execution(conversation.clone()).unwrap();
        let first =
            block_on(shared.get_or_create_session_binding(&resolved, Some("actor-a"))).unwrap();
        let restarted = ConversationService::new(repository.clone(), default_policy());
        let after_restart =
            block_on(restarted.get_or_create_session_binding(&resolved, Some("actor-b"))).unwrap();
        assert_eq!(first.session_id, after_restart.session_id);
        let advanced = block_on(shared.advance_session(&resolved, None, 0, 1)).unwrap();
        assert_eq!(advanced.session_version, 1);
        assert!(matches!(
            block_on(shared.advance_session(&resolved, None, 0, 2)),
            Err(ConversationError::SessionVersionConflict { actual: 1, .. })
        ));

        let mut actor_policy = default_policy();
        actor_policy.session_scope = AgentSessionScope::ActorInConversation;
        let actors = ConversationService::new(Arc::new(MemoryRepository::default()), actor_policy);
        let resolved = actors.resolve_execution(conversation).unwrap();
        let actor_a =
            block_on(actors.get_or_create_session_binding(&resolved, Some("actor-a"))).unwrap();
        let actor_b =
            block_on(actors.get_or_create_session_binding(&resolved, Some("actor-b"))).unwrap();
        assert_ne!(actor_a.session_id, actor_b.session_id);
    }

    #[test]
    fn policy_revision_is_persisted_without_changing_session_and_expire_fences_it() {
        let repository = Arc::new(MemoryRepository::default());
        let first_service = ConversationService::new(repository.clone(), default_policy());
        let conversation = group_conversation();
        let first_resolved = first_service
            .resolve_execution(conversation.clone())
            .unwrap();
        let first =
            block_on(first_service.get_or_create_session_binding(&first_resolved, None)).unwrap();

        let mut changed_policy = default_policy();
        changed_policy.revision = 2;
        let changed_service = ConversationService::new(repository.clone(), changed_policy);
        let changed_resolved = changed_service
            .resolve_execution(conversation.clone())
            .unwrap();
        let refreshed =
            block_on(changed_service.get_or_create_session_binding(&changed_resolved, None))
                .unwrap();
        assert_eq!(refreshed.session_id, first.session_id);
        assert_eq!(refreshed.policy_revision, 2);
        assert!(refreshed.generation > first.generation);

        let expired =
            block_on(changed_service.expire_session_binding(&changed_resolved, None)).unwrap();
        assert_ne!(expired.session_id, refreshed.session_id);
        assert_eq!(expired.session_version, 0);
        assert!(expired.generation > refreshed.generation);
    }

    fn group_conversation() -> QqConversationRef {
        QqConversationRef {
            version: QQ_CONVERSATION_REF_VERSION,
            account_id: "main".into(),
            kind: BotConversationKind::Group,
            user_id: None,
            group_id: Some("group".into()),
            guild_id: None,
            channel_id: None,
            thread_id: None,
        }
    }

    fn default_policy() -> ConversationPolicy {
        ConversationPolicy {
            revision: 1,
            session_scope: AgentSessionScope::SharedConversation,
            business_profile_binding_id: None,
            agent_runtime_profile_id: Some("default".into()),
            stt_enabled: false,
            tts_enabled: false,
            speech_reply_policy: mutsuki_bot_protocol::BotSpeechReplyPolicy::default(),
            stt_selector_id: None,
            tts_selector_id: None,
            active_delivery_enabled: false,
        }
    }
}
