use std::sync::Arc;

use mutsuki_bot_protocol::{
    AgentSessionBinding, AgentSessionScope, BotConversationKind, BotEvent, BotTarget,
    ConversationPolicy, ConversationPolicyLayer, ConversationPolicyPatch, ConversationPolicyRule,
    ConversationPolicyRuleSource, DirectMessagePolicy, MessageSegment, QQ_CONVERSATION_REF_VERSION,
    QqConversationRef, ResolvedConversationPolicy,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub trait ConversationRepository: Send + Sync {
    fn policy_rules(&self) -> Result<Vec<ConversationPolicyRule>, ConversationError>;

    fn session_binding(
        &self,
        binding_key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError>;

    fn compare_and_set_session_binding(
        &self,
        binding_key: &str,
        expected_generation: Option<u64>,
        binding: AgentSessionBinding,
    ) -> Result<(), ConversationError>;

    fn begin_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
        turn_id: &str,
    ) -> Result<AgentEventClaim, ConversationError>;

    fn complete_agent_event(
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

    pub fn resolve_policy(
        &self,
        conversation: QqConversationRef,
        actor_id: Option<&str>,
    ) -> Result<ResolvedConversationPolicy, ConversationError> {
        conversation
            .validate()
            .map_err(|error| ConversationError::InvalidConversationRef(error.to_string()))?;
        let origin_key = conversation.origin_key();
        let mut rules = self
            .repository
            .policy_rules()?
            .into_iter()
            .filter(|rule| rule_matches(rule, &conversation, &origin_key, actor_id))
            .collect::<Vec<_>>();
        rules.sort_by(|left, right| {
            rule_layer(left)
                .cmp(&rule_layer(right))
                .then_with(|| rule_specificity(left).cmp(&rule_specificity(right)))
                .then_with(|| left.revision.cmp(&right.revision))
                .then_with(|| left.rule_id.cmp(&right.rule_id))
        });
        let mut policy = self.product_default.clone();
        let mut matched_rule_ids = Vec::with_capacity(rules.len());
        let mut matched_rule_sources = Vec::with_capacity(rules.len());
        for rule in rules {
            let layer = rule_layer(&rule);
            matched_rule_sources.push(ConversationPolicyRuleSource {
                rule_id: rule.rule_id.clone(),
                layer,
                revision: rule.revision,
            });
            apply_patch(&mut policy, rule.revision, &rule.patch);
            matched_rule_ids.push(rule.rule_id);
        }
        Ok(ResolvedConversationPolicy {
            conversation,
            policy,
            matched_rule_ids,
            matched_rule_sources,
        })
    }

    pub fn admit_event(
        &self,
        resolved: &ResolvedConversationPolicy,
        event: &BotEvent,
    ) -> Result<(), ConversationAdmissionError> {
        let policy = &resolved.policy;
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        if !policy.enabled {
            return Err(ConversationAdmissionError::ConversationDisabled);
        }
        if actor_id.is_some_and(|actor| policy.denylist.iter().any(|entry| entry == actor)) {
            return Err(ConversationAdmissionError::ActorDenied);
        }
        if !policy.allowlist.is_empty()
            && !actor_id.is_some_and(|actor| policy.allowlist.iter().any(|entry| entry == actor))
        {
            return Err(ConversationAdmissionError::ActorNotAllowed);
        }
        if resolved.conversation.kind == BotConversationKind::Private {
            return match policy.direct_message_policy {
                DirectMessagePolicy::Allow => Ok(()),
                DirectMessagePolicy::Deny => Err(ConversationAdmissionError::DirectMessageDenied),
                DirectMessagePolicy::Allowlisted
                    if actor_id.is_some_and(|actor| {
                        policy.allowlist.iter().any(|entry| entry == actor)
                    }) =>
                {
                    Ok(())
                }
                DirectMessagePolicy::Allowlisted => {
                    Err(ConversationAdmissionError::DirectMessageNotAllowlisted)
                }
            };
        }
        if policy.must_mention || !policy.wake_words.is_empty() {
            let message = event
                .message
                .as_ref()
                .ok_or(ConversationAdmissionError::TriggerMissing)?;
            let mentioned = message.segments.iter().any(|segment| {
                matches!(segment, MessageSegment::MentionUser { user_id } if user_id == &event.bot.account_id)
            });
            let text = message.plain_text();
            let woke = policy
                .wake_words
                .iter()
                .any(|wake_word| !wake_word.is_empty() && text.contains(wake_word));
            if !mentioned && !woke {
                return Err(ConversationAdmissionError::TriggerMissing);
            }
        }
        Ok(())
    }

    pub fn get_or_create_session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<AgentSessionBinding, ConversationError> {
        let binding_key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        if let Some(binding) = self.repository.session_binding(&binding_key)? {
            if binding.policy_revision == resolved.policy.revision {
                return Ok(binding);
            }
            let refreshed = AgentSessionBinding {
                policy_revision: resolved.policy.revision,
                generation: binding.generation.saturating_add(1),
                ..binding.clone()
            };
            return match self.repository.compare_and_set_session_binding(
                &binding_key,
                Some(binding.generation),
                refreshed.clone(),
            ) {
                Ok(()) => Ok(refreshed),
                Err(ConversationError::GenerationConflict) => self
                    .repository
                    .session_binding(&binding_key)?
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
        {
            Ok(()) => Ok(binding),
            Err(ConversationError::GenerationConflict) => self
                .repository
                .session_binding(&binding_key)?
                .ok_or(ConversationError::GenerationConflict),
            Err(error) => Err(error),
        }
    }

    pub fn advance_session(
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
            .session_binding(&binding_key)?
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
        self.repository.compare_and_set_session_binding(
            &binding_key,
            Some(current.generation),
            next.clone(),
        )?;
        Ok(next)
    }

    pub fn advance_event_sequence(
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
            .session_binding(&binding_key)?
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
        self.repository.compare_and_set_session_binding(
            &binding_key,
            Some(current.generation),
            next.clone(),
        )?;
        Ok(next)
    }

    pub fn session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        let key = session_binding_key(
            &resolved.conversation,
            resolved.policy.session_scope,
            actor_id,
        )?;
        self.repository.session_binding(&key)
    }

    pub fn reset_session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<AgentSessionBinding, ConversationError> {
        self.replace_session_binding(resolved, actor_id, "reset")
    }

    pub fn expire_session_binding(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
    ) -> Result<AgentSessionBinding, ConversationError> {
        self.replace_session_binding(resolved, actor_id, "expire")
    }

    fn replace_session_binding(
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
            .session_binding(&key)?
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
        self.repository.compare_and_set_session_binding(
            &key,
            Some(current.generation),
            next.clone(),
        )?;
        Ok(next)
    }

    pub fn prepare_session_fork(
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
            .session_binding(&key)?
            .ok_or(ConversationError::BindingNotFound)?;
        let next_generation = current.generation.saturating_add(1);
        let target_session_id = stable_session_id(&format!("{key}|fork:{next_generation}"));
        Ok((current, target_session_id))
    }

    pub fn commit_session_fork(
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
            .session_binding(&key)?
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
        self.repository.compare_and_set_session_binding(
            &key,
            Some(current.generation),
            next.clone(),
        )?;
        Ok(next)
    }

    pub fn begin_agent_event(
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
        self.repository.begin_agent_event(&key, event_id, turn_id)
    }

    pub fn complete_agent_event(
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
        self.repository.complete_agent_event(&key, event_id)
    }
}

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

fn rule_matches(
    rule: &ConversationPolicyRule,
    conversation: &QqConversationRef,
    origin_key: &str,
    actor_id: Option<&str>,
) -> bool {
    let matcher = &rule.matcher;
    matcher
        .account_id
        .as_ref()
        .is_none_or(|value| value == &conversation.account_id)
        && matcher
            .kind
            .as_ref()
            .is_none_or(|value| value == &conversation.kind)
        && matcher
            .group_id
            .as_ref()
            .is_none_or(|value| Some(value) == conversation.group_id.as_ref())
        && matcher
            .guild_id
            .as_ref()
            .is_none_or(|value| Some(value) == conversation.guild_id.as_ref())
        && matcher
            .channel_id
            .as_ref()
            .is_none_or(|value| Some(value) == conversation.channel_id.as_ref())
        && matcher
            .origin_key
            .as_ref()
            .is_none_or(|value| value == origin_key)
        && matcher
            .actor_id
            .as_deref()
            .is_none_or(|value| Some(value) == actor_id)
}

fn rule_layer(rule: &ConversationPolicyRule) -> ConversationPolicyLayer {
    let matcher = &rule.matcher;
    if matcher.actor_id.is_some() {
        ConversationPolicyLayer::ActorInConversation
    } else if matcher.origin_key.is_some() {
        ConversationPolicyLayer::Conversation
    } else if matcher.channel_id.is_some() {
        ConversationPolicyLayer::Channel
    } else if matcher.group_id.is_some() {
        ConversationPolicyLayer::Group
    } else if matcher.guild_id.is_some() {
        ConversationPolicyLayer::Guild
    } else {
        ConversationPolicyLayer::Account
    }
}

fn rule_specificity(rule: &ConversationPolicyRule) -> usize {
    let matcher = &rule.matcher;
    [
        matcher.account_id.is_some(),
        matcher.kind.is_some(),
        matcher.group_id.is_some(),
        matcher.guild_id.is_some(),
        matcher.channel_id.is_some(),
        matcher.origin_key.is_some(),
        matcher.actor_id.is_some(),
    ]
    .into_iter()
    .filter(|matches| *matches)
    .count()
}

fn apply_patch(policy: &mut ConversationPolicy, revision: u64, patch: &ConversationPolicyPatch) {
    policy.revision = policy.revision.max(revision);
    set(&mut policy.enabled, patch.enabled);
    set(&mut policy.agent_enabled, patch.agent_enabled);
    set(
        &mut policy.direct_message_policy,
        patch.direct_message_policy,
    );
    set(&mut policy.must_mention, patch.must_mention);
    set(&mut policy.wake_words, patch.wake_words.clone());
    set(&mut policy.allowlist, patch.allowlist.clone());
    set(&mut policy.denylist, patch.denylist.clone());
    set(
        &mut policy.rate_limit_profile_id,
        patch.rate_limit_profile_id.clone(),
    );
    set(&mut policy.session_scope, patch.session_scope);
    set(
        &mut policy.business_profile_binding_id,
        patch.business_profile_binding_id.clone(),
    );
    set(
        &mut policy.agent_runtime_profile_id,
        patch.agent_runtime_profile_id.clone(),
    );
    set(&mut policy.stt_enabled, patch.stt_enabled);
    set(&mut policy.tts_enabled, patch.tts_enabled);
    set(&mut policy.speech_reply_policy, patch.speech_reply_policy);
    set(&mut policy.stt_selector_id, patch.stt_selector_id.clone());
    set(&mut policy.tts_selector_id, patch.tts_selector_id.clone());
    set(
        &mut policy.active_delivery_enabled,
        patch.active_delivery_enabled,
    );
}

fn set<T>(target: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *target = value;
    }
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

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ConversationAdmissionError {
    #[error("conversation is disabled")]
    ConversationDisabled,
    #[error("actor is denied by conversation policy")]
    ActorDenied,
    #[error("actor is not allowed by conversation policy")]
    ActorNotAllowed,
    #[error("direct messages are disabled by conversation policy")]
    DirectMessageDenied,
    #[error("direct-message actor is not allowlisted")]
    DirectMessageNotAllowlisted,
    #[error("message does not mention the bot or contain a configured wake word")]
    TriggerMissing,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use mutsuki_bot_protocol::{
        BotAccountRef, BotConversationKind, BotEventKind, BotMessage, BotPlatform, BotUser,
        ConversationPolicyMatch, QQ_CONVERSATION_REF_VERSION,
    };

    use super::*;

    #[derive(Default)]
    struct MemoryRepository {
        rules: Vec<ConversationPolicyRule>,
        bindings: Mutex<BTreeMap<String, AgentSessionBinding>>,
        events: Mutex<BTreeMap<(String, String), bool>>,
    }

    impl ConversationRepository for MemoryRepository {
        fn policy_rules(&self) -> Result<Vec<ConversationPolicyRule>, ConversationError> {
            Ok(self.rules.clone())
        }

        fn session_binding(
            &self,
            binding_key: &str,
        ) -> Result<Option<AgentSessionBinding>, ConversationError> {
            Ok(self.bindings.lock().unwrap().get(binding_key).cloned())
        }

        fn compare_and_set_session_binding(
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

        fn begin_agent_event(
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

        fn complete_agent_event(
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
    fn policy_precedence_is_deterministic_and_reports_sources() {
        let repository = Arc::new(MemoryRepository {
            rules: vec![
                rule(
                    "account",
                    ConversationPolicyMatch {
                        account_id: Some("main".into()),
                        ..Default::default()
                    },
                    ConversationPolicyPatch {
                        must_mention: Some(true),
                        ..Default::default()
                    },
                ),
                rule(
                    "group",
                    ConversationPolicyMatch {
                        account_id: Some("main".into()),
                        group_id: Some("group".into()),
                        ..Default::default()
                    },
                    ConversationPolicyPatch {
                        agent_enabled: Some(true),
                        must_mention: Some(false),
                        ..Default::default()
                    },
                ),
            ],
            ..Default::default()
        });
        let resolved = ConversationService::new(repository, default_policy())
            .resolve_policy(group_conversation(), Some("actor"))
            .unwrap();
        assert!(resolved.policy.agent_enabled);
        assert!(!resolved.policy.must_mention);
        assert_eq!(resolved.matched_rule_ids, ["account", "group"]);
        assert_eq!(
            resolved
                .matched_rule_sources
                .iter()
                .map(|source| source.layer)
                .collect::<Vec<_>>(),
            [
                mutsuki_bot_protocol::ConversationPolicyLayer::Account,
                mutsuki_bot_protocol::ConversationPolicyLayer::Group,
            ]
        );
    }

    #[test]
    fn policy_hierarchy_wins_over_rule_id_order() {
        let conversation = group_conversation();
        let repository = Arc::new(MemoryRepository {
            rules: vec![
                rule(
                    "z-account",
                    ConversationPolicyMatch {
                        account_id: Some("main".into()),
                        ..Default::default()
                    },
                    ConversationPolicyPatch {
                        must_mention: Some(true),
                        ..Default::default()
                    },
                ),
                rule(
                    "a-conversation",
                    ConversationPolicyMatch {
                        origin_key: Some(conversation.origin_key()),
                        ..Default::default()
                    },
                    ConversationPolicyPatch {
                        must_mention: Some(false),
                        ..Default::default()
                    },
                ),
                rule(
                    "0-actor",
                    ConversationPolicyMatch {
                        actor_id: Some("actor".into()),
                        ..Default::default()
                    },
                    ConversationPolicyPatch {
                        must_mention: Some(true),
                        ..Default::default()
                    },
                ),
            ],
            ..Default::default()
        });
        let resolved = ConversationService::new(repository, default_policy())
            .resolve_policy(conversation, Some("actor"))
            .unwrap();

        assert!(resolved.policy.must_mention);
        assert_eq!(
            resolved.matched_rule_ids,
            ["z-account", "a-conversation", "0-actor"]
        );
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
        let resolved = shared
            .resolve_policy(conversation.clone(), Some("actor-a"))
            .unwrap();
        let first = shared
            .get_or_create_session_binding(&resolved, Some("actor-a"))
            .unwrap();
        let after_restart = ConversationService::new(repository.clone(), default_policy())
            .get_or_create_session_binding(&resolved, Some("actor-b"))
            .unwrap();
        assert_eq!(first.session_id, after_restart.session_id);
        let advanced = shared.advance_session(&resolved, None, 0, 1).unwrap();
        assert_eq!(advanced.session_version, 1);
        assert!(matches!(
            shared.advance_session(&resolved, None, 0, 2),
            Err(ConversationError::SessionVersionConflict { actual: 1, .. })
        ));

        let mut actor_policy = default_policy();
        actor_policy.session_scope = AgentSessionScope::ActorInConversation;
        let actors = ConversationService::new(Arc::new(MemoryRepository::default()), actor_policy);
        let resolved = actors.resolve_policy(conversation, None).unwrap();
        let actor_a = actors
            .get_or_create_session_binding(&resolved, Some("actor-a"))
            .unwrap();
        let actor_b = actors
            .get_or_create_session_binding(&resolved, Some("actor-b"))
            .unwrap();
        assert_ne!(actor_a.session_id, actor_b.session_id);
    }

    #[test]
    fn policy_revision_is_persisted_without_changing_session_and_expire_fences_it() {
        let repository = Arc::new(MemoryRepository::default());
        let first_service = ConversationService::new(repository.clone(), default_policy());
        let conversation = group_conversation();
        let first_resolved = first_service
            .resolve_policy(conversation.clone(), None)
            .unwrap();
        let first = first_service
            .get_or_create_session_binding(&first_resolved, None)
            .unwrap();

        let mut changed_policy = default_policy();
        changed_policy.revision = 2;
        let changed_service = ConversationService::new(repository.clone(), changed_policy);
        let changed_resolved = changed_service
            .resolve_policy(conversation.clone(), None)
            .unwrap();
        let refreshed = changed_service
            .get_or_create_session_binding(&changed_resolved, None)
            .unwrap();
        assert_eq!(refreshed.session_id, first.session_id);
        assert_eq!(refreshed.policy_revision, 2);
        assert!(refreshed.generation > first.generation);

        let expired = changed_service
            .expire_session_binding(&changed_resolved, None)
            .unwrap();
        assert_ne!(expired.session_id, refreshed.session_id);
        assert_eq!(expired.session_version, 0);
        assert!(expired.generation > refreshed.generation);
    }

    #[test]
    fn admission_enforces_actor_direct_message_and_group_trigger_policy() {
        let service = ConversationService::new(
            Arc::new(MemoryRepository::default()),
            ConversationPolicy {
                allowlist: vec!["allowed".into()],
                denylist: vec!["denied".into()],
                direct_message_policy: DirectMessagePolicy::Allowlisted,
                must_mention: true,
                wake_words: vec!["mutsuki".into()],
                ..default_policy()
            },
        );

        let direct_denied = bot_event(
            BotTarget::User {
                user_id: "denied".into(),
            },
            "denied",
            vec![MessageSegment::text("hello")],
        );
        let resolved = service
            .resolve_policy(
                qq_conversation_from_event(&direct_denied).unwrap(),
                Some("denied"),
            )
            .unwrap();
        assert_eq!(
            service.admit_event(&resolved, &direct_denied),
            Err(ConversationAdmissionError::ActorDenied)
        );

        let group_unaddressed = bot_event(
            BotTarget::Group {
                group_id: "group".into(),
            },
            "allowed",
            vec![MessageSegment::text("hello")],
        );
        let resolved = service
            .resolve_policy(
                qq_conversation_from_event(&group_unaddressed).unwrap(),
                Some("allowed"),
            )
            .unwrap();
        assert_eq!(
            service.admit_event(&resolved, &group_unaddressed),
            Err(ConversationAdmissionError::TriggerMissing)
        );

        let group_woken = bot_event(
            BotTarget::Group {
                group_id: "group".into(),
            },
            "allowed",
            vec![MessageSegment::text("mutsuki help")],
        );
        assert_eq!(service.admit_event(&resolved, &group_woken), Ok(()));

        let group_mentioned = bot_event(
            BotTarget::Group {
                group_id: "group".into(),
            },
            "allowed",
            vec![MessageSegment::MentionUser {
                user_id: "main".into(),
            }],
        );
        assert_eq!(service.admit_event(&resolved, &group_mentioned), Ok(()));
    }

    fn rule(
        rule_id: &str,
        matcher: ConversationPolicyMatch,
        patch: ConversationPolicyPatch,
    ) -> ConversationPolicyRule {
        ConversationPolicyRule {
            rule_id: rule_id.into(),
            revision: 2,
            matcher,
            patch,
        }
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

    fn bot_event(target: BotTarget, actor_id: &str, segments: Vec<MessageSegment>) -> BotEvent {
        BotEvent {
            event_id: "event".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: target.clone(),
            actor: Some(BotUser {
                user_id: actor_id.into(),
                display_name: None,
                avatar_url: None,
            }),
            message: Some(BotMessage {
                message_id: Some("message".into()),
                target,
                sender: None,
                segments,
                reply_to: None,
                time_ms: Some(1),
                ext: BTreeMap::default(),
            }),
            raw: None,
            ext: BTreeMap::default(),
        }
    }

    fn default_policy() -> ConversationPolicy {
        ConversationPolicy {
            revision: 1,
            enabled: true,
            agent_enabled: false,
            direct_message_policy: DirectMessagePolicy::Allow,
            must_mention: false,
            wake_words: Vec::new(),
            allowlist: Vec::new(),
            denylist: Vec::new(),
            rate_limit_profile_id: None,
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
