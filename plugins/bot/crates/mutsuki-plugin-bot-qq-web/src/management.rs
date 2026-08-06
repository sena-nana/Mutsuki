//! Production QQ management owner: revision-fenced writes, audit, and live provider injection.

use std::sync::{Arc, Mutex};

use mutsuki_bot_protocol::{
    BotCommandDescriptor, BotDeliveryAttempt, BotDeliveryReceipt, BotHandlerDescriptor,
    BotInteractionSession, ConversationPolicy, DeliveryStatus, InteractionStatus,
    QqBotCapabilityMatrix, QqStreamingStrategy,
};
use serde_json::{Value, json};

use crate::{
    QqAccountView, QqAgentSessionView, QqBotManagementApi, QqBotManagementSnapshot,
    QqConversationView, QqDeliveryView, QqGatewayConnectionState, QqHandlerView, QqManagementAction,
    QqManagementError, QqManagementWriteRequest, QqManagementWriteResult,
};

/// Live snapshot/action backend injected into [`QqBotManagementService`].
pub trait QqManagementProvider: Send + Sync {
    /// Loads the current management projection.
    ///
    /// # Errors
    ///
    /// Returns a typed management error when the owner cannot materialize the snapshot.
    fn load_snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError>;

    /// Applies one management action against the live owner.
    ///
    /// # Errors
    ///
    /// Returns a typed management error when the action is rejected or the owner fails.
    fn apply(
        &self,
        actor_id: &str,
        action: &QqManagementAction,
    ) -> Result<Value, QqManagementError>;
}

#[derive(Clone, Debug)]
struct AuditEntry {
    audit_id: String,
    actor_id: String,
    action: String,
    revision: u64,
}

/// Production management service: revision fence + audit around an injected provider.
pub struct QqBotManagementService {
    revision: Mutex<u64>,
    audits: Mutex<Vec<AuditEntry>>,
    provider: Arc<dyn QqManagementProvider>,
}

impl QqBotManagementService {
    #[must_use]
    pub fn new(provider: Arc<dyn QqManagementProvider>) -> Self {
        Self {
            revision: Mutex::new(0),
            audits: Mutex::new(Vec::new()),
            provider,
        }
    }

    /// Builds a service backed by the in-process local provider.
    #[must_use]
    pub fn local(provider: LocalQqManagementProvider) -> Self {
        Self::new(Arc::new(provider))
    }

    #[must_use]
    pub fn audits(&self) -> Vec<(String, String, String, u64)> {
        self.audits
            .lock()
            .expect("qq management audit mutex")
            .iter()
            .map(|entry| {
                (
                    entry.audit_id.clone(),
                    entry.actor_id.clone(),
                    entry.action.clone(),
                    entry.revision,
                )
            })
            .collect()
    }
}

impl QqBotManagementApi for QqBotManagementService {
    fn snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError> {
        let mut snapshot = self.provider.load_snapshot(query, include_secret_status)?;
        snapshot.revision = *self.revision.lock().expect("qq management revision mutex");
        Ok(snapshot)
    }

    fn write(
        &self,
        request: QqManagementWriteRequest,
    ) -> Result<QqManagementWriteResult, QqManagementError> {
        let mut revision = self.revision.lock().expect("qq management revision mutex");
        if request.expected_revision != *revision {
            return Err(QqManagementError {
                code: "revision.conflict".into(),
                message: format!(
                    "expected revision {}, current {}",
                    request.expected_revision, *revision
                ),
            });
        }
        let result = self.provider.apply(&request.actor_id, &request.action)?;
        *revision = revision.saturating_add(1);
        let next = *revision;
        let audit_id = format!("audit-{next}");
        self.audits
            .lock()
            .expect("qq management audit mutex")
            .push(AuditEntry {
                audit_id: audit_id.clone(),
                actor_id: request.actor_id,
                action: action_name(&request.action).into(),
                revision: next,
            });
        Ok(QqManagementWriteResult {
            revision: next,
            audit_id,
            result,
        })
    }
}

/// In-process management owner used when a product has not replaced the live provider.
#[derive(Default)]
pub struct LocalQqManagementProvider {
    state: Mutex<LocalState>,
}

#[derive(Clone, Default)]
struct LocalState {
    accounts: Vec<QqAccountView>,
    conversations: Vec<QqConversationView>,
    handlers: Vec<QqHandlerView>,
    commands: Vec<BotCommandDescriptor>,
    agent_sessions: Vec<QqAgentSessionView>,
    deliveries: Vec<QqDeliveryView>,
    interactions: Vec<BotInteractionSession>,
}

impl LocalQqManagementProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_account(&self, account: QqAccountView) {
        let mut state = self.state.lock().expect("qq local management mutex");
        if let Some(existing) = state
            .accounts
            .iter_mut()
            .find(|item| item.account_id == account.account_id)
        {
            *existing = account;
        } else {
            state.accounts.push(account);
        }
    }

    pub fn replace_handlers(&self, handlers: Vec<QqHandlerView>) {
        self.state.lock().expect("qq local management mutex").handlers = handlers;
    }

    pub fn replace_commands(&self, commands: Vec<BotCommandDescriptor>) {
        self.state.lock().expect("qq local management mutex").commands = commands;
    }

    pub fn upsert_conversation(&self, conversation: QqConversationView) {
        let mut state = self.state.lock().expect("qq local management mutex");
        if let Some(existing) = state
            .conversations
            .iter_mut()
            .find(|item| item.conversation == conversation.conversation)
        {
            *existing = conversation;
        } else {
            state.conversations.push(conversation);
        }
    }

    pub fn upsert_agent_session(&self, session: QqAgentSessionView) {
        let mut state = self.state.lock().expect("qq local management mutex");
        if let Some(existing) = state
            .agent_sessions
            .iter_mut()
            .find(|item| item.binding.origin_key == session.binding.origin_key)
        {
            *existing = session;
        } else {
            state.agent_sessions.push(session);
        }
    }

    pub fn upsert_delivery(&self, delivery: QqDeliveryView) {
        let mut state = self.state.lock().expect("qq local management mutex");
        if let Some(existing) = state
            .deliveries
            .iter_mut()
            .find(|item| item.receipt.delivery_id == delivery.receipt.delivery_id)
        {
            *existing = delivery;
        } else {
            state.deliveries.push(delivery);
        }
    }

    pub fn upsert_interaction(&self, session: BotInteractionSession) {
        let mut state = self.state.lock().expect("qq local management mutex");
        if let Some(existing) = state
            .interactions
            .iter_mut()
            .find(|item| item.session_id == session.session_id)
        {
            *existing = session;
        } else {
            state.interactions.push(session);
        }
    }
}

impl QqManagementProvider for LocalQqManagementProvider {
    fn load_snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError> {
        let state = self.state.lock().expect("qq local management mutex").clone();
        let query = query.trim().to_ascii_lowercase();
        let accounts = state
            .accounts
            .into_iter()
            .filter(|account| query.is_empty() || account.account_id.to_ascii_lowercase().contains(&query))
            .map(|mut account| {
                if !include_secret_status {
                    account.credential_reference.clear();
                    account.credential_status = "restricted".into();
                }
                account
            })
            .collect();
        let conversations = state
            .conversations
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item
                        .conversation
                        .origin_key()
                        .to_ascii_lowercase()
                        .contains(&query)
                    || item.matched_rule_ids.iter().any(|rule| {
                        rule.to_ascii_lowercase().contains(&query)
                    })
            })
            .collect();
        let deliveries = state
            .deliveries
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item
                        .receipt
                        .delivery_id
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .collect();
        Ok(QqBotManagementSnapshot {
            revision: 0,
            accounts,
            conversations,
            handlers: state.handlers,
            commands: state.commands,
            agent_sessions: state.agent_sessions,
            deliveries,
            interactions: state.interactions,
        })
    }

    fn apply(
        &self,
        _actor_id: &str,
        action: &QqManagementAction,
    ) -> Result<Value, QqManagementError> {
        let mut state = self.state.lock().expect("qq local management mutex");
        match action {
            QqManagementAction::AccountSetEnabled {
                account_id,
                enabled,
            } => {
                let account = account_mut(&mut state, account_id)?;
                account.enabled = *enabled;
                Ok(json!({ "account_id": account_id, "enabled": enabled }))
            }
            QqManagementAction::AccountHealthCheck { account_id } => {
                let account = account_mut(&mut state, account_id)?;
                account.health = match account.connection_state {
                    QqGatewayConnectionState::Identified | QqGatewayConnectionState::Resumable => {
                        "ok".into()
                    }
                    QqGatewayConnectionState::Connected => "degraded".into(),
                    QqGatewayConnectionState::Disconnected => "unhealthy".into(),
                };
                Ok(json!({
                    "account_id": account_id,
                    "health": account.health,
                    "connection_state": account.connection_state,
                }))
            }
            QqManagementAction::AccountReconnect { account_id } => {
                let account = account_mut(&mut state, account_id)?;
                account.connection_state = QqGatewayConnectionState::Connected;
                account.health = "reconnecting".into();
                account.rate_limit_status = "ready".into();
                Ok(json!({ "account_id": account_id, "reconnected": true }))
            }
            QqManagementAction::AccountSendTest {
                account_id,
                conversation,
                text,
            } => {
                let _ = account_mut(&mut state, account_id)?;
                if text.trim().is_empty() {
                    return Err(QqManagementError {
                        code: "invalid_argument".into(),
                        message: "test text must not be empty".into(),
                    });
                }
                Ok(json!({
                    "account_id": account_id,
                    "conversation": conversation,
                    "sent": true,
                    "preview": text,
                }))
            }
            QqManagementAction::ConversationUpdate {
                conversation,
                policy,
            } => {
                let next_policy = ConversationPolicy {
                    revision: policy.revision.saturating_add(1),
                    ..policy.clone()
                };
                if let Some(existing) = state
                    .conversations
                    .iter_mut()
                    .find(|item| &item.conversation == conversation)
                {
                    existing.policy = next_policy.clone();
                    if !existing.matched_rule_ids.iter().any(|id| id == "console") {
                        existing.matched_rule_ids.push("console".into());
                    }
                } else {
                    state.conversations.push(QqConversationView {
                        conversation: conversation.clone(),
                        policy: next_policy.clone(),
                        matched_rule_ids: vec!["console".into()],
                    });
                }
                Ok(json!({
                    "conversation": conversation,
                    "policy": next_policy,
                    "matched_rule_ids": ["console"],
                }))
            }
            QqManagementAction::HandlerSetEnabled {
                handler_id,
                generation,
                enabled,
            } => {
                let handler = state
                    .handlers
                    .iter_mut()
                    .find(|item| item.descriptor.handler_id == *handler_id)
                    .ok_or_else(|| QqManagementError {
                        code: "not_found".into(),
                        message: format!("handler `{handler_id}` was not found"),
                    })?;
                if handler.descriptor.generation != *generation {
                    return Err(QqManagementError {
                        code: "generation.conflict".into(),
                        message: format!(
                            "handler `{handler_id}` generation mismatch: expected {}, got {}",
                            handler.descriptor.generation, generation
                        ),
                    });
                }
                handler.enabled = *enabled;
                Ok(json!({
                    "handler_id": handler_id,
                    "generation": generation,
                    "enabled": enabled,
                }))
            }
            QqManagementAction::AgentCancel {
                origin_key,
                turn_id,
            } => {
                let session = agent_mut(&mut state, origin_key)?;
                if session.current_turn_id.as_deref() != Some(turn_id.as_str()) {
                    return Err(QqManagementError {
                        code: "not_found".into(),
                        message: format!("turn `{turn_id}` is not active for `{origin_key}`"),
                    });
                }
                session.current_turn_id = None;
                session.status = "cancelled".into();
                Ok(json!({ "origin_key": origin_key, "turn_id": turn_id, "cancelled": true }))
            }
            QqManagementAction::AgentReset { origin_key } => {
                let session = agent_mut(&mut state, origin_key)?;
                session.binding.session_version = session.binding.session_version.saturating_add(1);
                session.binding.generation = session.binding.generation.saturating_add(1);
                session.current_turn_id = None;
                session.status = "reset".into();
                Ok(json!({
                    "origin_key": origin_key,
                    "session_id": session.binding.session_id,
                    "session_version": session.binding.session_version,
                }))
            }
            QqManagementAction::AgentFork { origin_key } => {
                let session = agent_mut(&mut state, origin_key)?;
                let forked_id = format!("{}:fork:{}", session.binding.session_id, session.binding.generation);
                session.binding.session_id = forked_id.clone();
                session.binding.session_version = 1;
                session.binding.generation = session.binding.generation.saturating_add(1);
                session.status = "forked".into();
                session.current_turn_id = None;
                Ok(json!({ "origin_key": origin_key, "session_id": forked_id }))
            }
            QqManagementAction::AgentRegenerate { origin_key } => {
                let session = agent_mut(&mut state, origin_key)?;
                let turn_id = format!("regen-{}", session.binding.session_version.saturating_add(1));
                session.current_turn_id = Some(turn_id.clone());
                session.status = "regenerating".into();
                Ok(json!({ "origin_key": origin_key, "turn_id": turn_id }))
            }
            QqManagementAction::DeliveryRetry { delivery_id } => {
                let delivery = delivery_mut(&mut state, delivery_id)?;
                match delivery.receipt.status {
                    DeliveryStatus::RetryScheduled
                    | DeliveryStatus::PermanentlyFailed
                    | DeliveryStatus::ReconcileRequired
                    | DeliveryStatus::Cancelled => {}
                    other => {
                        return Err(QqManagementError {
                            code: "invalid_state".into(),
                            message: format!("delivery `{delivery_id}` status {other:?} cannot retry"),
                        });
                    }
                }
                let attempt = delivery.receipt.attempt_count.saturating_add(1);
                delivery.receipt.attempt_count = attempt;
                delivery.receipt.status = DeliveryStatus::Pending;
                delivery.receipt.error_code = None;
                delivery.attempts.push(BotDeliveryAttempt {
                    delivery_id: delivery_id.clone(),
                    attempt,
                    status: DeliveryStatus::Pending,
                    started_at_unix_ms: 0,
                    retry_at_unix_ms: None,
                    error_code: None,
                });
                Ok(json!({ "delivery_id": delivery_id, "attempt_count": attempt }))
            }
            QqManagementAction::DeliveryCancel { delivery_id } => {
                let delivery = delivery_mut(&mut state, delivery_id)?;
                delivery.receipt.status = DeliveryStatus::Cancelled;
                Ok(json!({ "delivery_id": delivery_id, "status": "cancelled" }))
            }
            QqManagementAction::DeliveryPreview { delivery_id } => {
                let delivery = delivery_mut(&mut state, delivery_id)?;
                delivery.receipt.status = DeliveryStatus::Previewed;
                Ok(json!({
                    "delivery_id": delivery_id,
                    "status": "previewed",
                    "attempt_count": delivery.receipt.attempt_count,
                }))
            }
            QqManagementAction::InteractionCancel { session_id } => {
                let session = state
                    .interactions
                    .iter_mut()
                    .find(|item| item.session_id == *session_id)
                    .ok_or_else(|| QqManagementError {
                        code: "not_found".into(),
                        message: format!("interaction `{session_id}` was not found"),
                    })?;
                if session.status != InteractionStatus::Waiting {
                    return Err(QqManagementError {
                        code: "invalid_state".into(),
                        message: format!("interaction `{session_id}` is not waiting"),
                    });
                }
                session.status = InteractionStatus::Cancelled;
                session.version = session.version.saturating_add(1);
                Ok(json!({
                    "session_id": session_id,
                    "status": "cancelled",
                    "version": session.version,
                }))
            }
        }
    }
}

/// Builds an account view from adapter config facts plus live gateway health.
#[must_use]
pub fn account_view_from_config(
    account_id: impl Into<String>,
    credential_reference: impl Into<String>,
    credential_present: bool,
    capability: QqBotCapabilityMatrix,
    intents: u64,
    shard: [u64; 2],
    connected: bool,
    identified: bool,
    last_heartbeat_unix_ms: Option<u64>,
    last_error: Option<&str>,
) -> QqAccountView {
    let connection_state = if identified {
        QqGatewayConnectionState::Identified
    } else if connected {
        QqGatewayConnectionState::Connected
    } else if last_error.is_some() {
        QqGatewayConnectionState::Resumable
    } else {
        QqGatewayConnectionState::Disconnected
    };
    let health = if identified && connected {
        "ok"
    } else if connected {
        "degraded"
    } else if last_error.is_some() {
        "unhealthy"
    } else {
        "stopped"
    };
    QqAccountView {
        account_id: account_id.into(),
        enabled: true,
        health: health.into(),
        connection_state,
        last_heartbeat_unix_ms,
        intents,
        shard,
        credential_reference: credential_reference.into(),
        credential_status: if credential_present {
            "configured"
        } else {
            "absent"
        }
        .into(),
        rate_limit_status: "ready".into(),
        capability,
    }
}

#[must_use]
pub fn handler_view(descriptor: BotHandlerDescriptor, enabled: bool) -> QqHandlerView {
    QqHandlerView {
        descriptor,
        enabled,
        rate_limit_status: "ready".into(),
        last_error_code: None,
        last_invocation_trace_id: None,
    }
}

#[must_use]
pub fn agent_session_view(
    binding: mutsuki_bot_protocol::AgentSessionBinding,
    streaming: QqStreamingStrategy,
) -> QqAgentSessionView {
    QqAgentSessionView {
        binding,
        status: "idle".into(),
        current_turn_id: None,
        streaming,
        approval_status: None,
        delivery_status: None,
    }
}

#[must_use]
pub fn delivery_view(receipt: BotDeliveryReceipt, attempts: Vec<BotDeliveryAttempt>) -> QqDeliveryView {
    QqDeliveryView { receipt, attempts }
}

fn account_mut<'a>(
    state: &'a mut LocalState,
    account_id: &str,
) -> Result<&'a mut QqAccountView, QqManagementError> {
    state
        .accounts
        .iter_mut()
        .find(|item| item.account_id == account_id)
        .ok_or_else(|| QqManagementError {
            code: "not_found".into(),
            message: format!("account `{account_id}` was not found"),
        })
}

fn agent_mut<'a>(
    state: &'a mut LocalState,
    origin_key: &str,
) -> Result<&'a mut QqAgentSessionView, QqManagementError> {
    state
        .agent_sessions
        .iter_mut()
        .find(|item| item.binding.origin_key == origin_key)
        .ok_or_else(|| QqManagementError {
            code: "not_found".into(),
            message: format!("agent session `{origin_key}` was not found"),
        })
}

fn delivery_mut<'a>(
    state: &'a mut LocalState,
    delivery_id: &str,
) -> Result<&'a mut QqDeliveryView, QqManagementError> {
    state
        .deliveries
        .iter_mut()
        .find(|item| item.receipt.delivery_id == delivery_id)
        .ok_or_else(|| QqManagementError {
            code: "not_found".into(),
            message: format!("delivery `{delivery_id}` was not found"),
        })
}

fn action_name(action: &QqManagementAction) -> &'static str {
    match action {
        QqManagementAction::AccountSetEnabled { .. } => "account_set_enabled",
        QqManagementAction::AccountHealthCheck { .. } => "account_health_check",
        QqManagementAction::AccountReconnect { .. } => "account_reconnect",
        QqManagementAction::AccountSendTest { .. } => "account_send_test",
        QqManagementAction::ConversationUpdate { .. } => "conversation_update",
        QqManagementAction::HandlerSetEnabled { .. } => "handler_set_enabled",
        QqManagementAction::AgentCancel { .. } => "agent_cancel",
        QqManagementAction::AgentReset { .. } => "agent_reset",
        QqManagementAction::AgentFork { .. } => "agent_fork",
        QqManagementAction::AgentRegenerate { .. } => "agent_regenerate",
        QqManagementAction::DeliveryRetry { .. } => "delivery_retry",
        QqManagementAction::DeliveryCancel { .. } => "delivery_cancel",
        QqManagementAction::DeliveryPreview { .. } => "delivery_preview",
        QqManagementAction::InteractionCancel { .. } => "interaction_cancel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        AgentSessionBinding, AgentSessionScope, BotConversationKind, BotPropagationPolicy,
        DirectMessagePolicy, InteractionScope, InteractionWaitSpec, QqMessageSegmentKind,
        QqPermissionRequirement, QqRateLimitPolicy, QqUploadConstraints,
    };

    fn capability(account_id: &str) -> QqBotCapabilityMatrix {
        QqBotCapabilityMatrix {
            account_id: account_id.into(),
            conversation_kinds: vec![BotConversationKind::Private],
            outbound_conversation_kinds: vec![BotConversationKind::Private],
            active_message_kinds: vec![BotConversationKind::Private],
            inbound_segments: vec![QqMessageSegmentKind::Text],
            outbound_segments: vec![QqMessageSegmentKind::Text],
            inbound_media: Vec::new(),
            outbound_media: Vec::new(),
            active_message: true,
            message_edit: false,
            message_recall: true,
            reply: true,
            quote: true,
            mention: true,
            upload: QqUploadConstraints::default(),
            rate_limit: QqRateLimitPolicy::default(),
            streaming: vec![QqStreamingStrategy::FinalOnly],
            configured_intents: 1,
            shard: [0, 1],
            required_intents: vec!["group_and_c2c_event".into()],
            required_permissions: vec![QqPermissionRequirement::ReadC2cMessages],
        }
    }

    fn conversation() -> mutsuki_bot_protocol::QqConversationRef {
        mutsuki_bot_protocol::QqConversationRef {
            version: mutsuki_bot_protocol::QQ_CONVERSATION_REF_VERSION,
            kind: BotConversationKind::Private,
            account_id: "main".into(),
            user_id: Some("u1".into()),
            group_id: None,
            guild_id: None,
            channel_id: None,
            thread_id: None,
        }
    }

    fn policy() -> ConversationPolicy {
        ConversationPolicy {
            revision: 1,
            enabled: true,
            agent_enabled: true,
            direct_message_policy: DirectMessagePolicy::Allow,
            must_mention: false,
            wake_words: Vec::new(),
            allowlist: Vec::new(),
            denylist: Vec::new(),
            rate_limit_profile_id: None,
            session_scope: AgentSessionScope::SharedConversation,
            business_profile_binding_id: None,
            agent_runtime_profile_id: None,
            stt_enabled: false,
            tts_enabled: false,
            speech_reply_policy: Default::default(),
            stt_selector_id: None,
            tts_selector_id: None,
            active_delivery_enabled: true,
        }
    }

    fn service() -> (QqBotManagementService, Arc<LocalQqManagementProvider>) {
        let local = Arc::new(LocalQqManagementProvider::new());
        local.upsert_account(account_view_from_config(
            "main",
            "QQBOT_CLIENT_SECRET",
            true,
            capability("main"),
            1,
            [0, 1],
            true,
            true,
            Some(10),
            None,
        ));
        local.upsert_conversation(QqConversationView {
            conversation: conversation(),
            policy: policy(),
            matched_rule_ids: vec!["product".into()],
        });
        local.upsert_agent_session(agent_session_view(
            AgentSessionBinding {
                origin_key: conversation().origin_key(),
                session_id: "sess-1".into(),
                session_version: 3,
                last_event_sequence: 1,
                policy_revision: 1,
                generation: 2,
            },
            QqStreamingStrategy::FinalOnly,
        ));
        local.upsert_delivery(delivery_view(
            BotDeliveryReceipt {
                delivery_id: "d1".into(),
                idempotency_key: "k1".into(),
                status: DeliveryStatus::RetryScheduled,
                attempt_count: 1,
                platform_message_ids: Vec::new(),
                part_receipts: Vec::new(),
                delivered_at_unix_ms: None,
                error_code: Some("transient".into()),
                generation: 1,
                lease_expires_at_unix_ms: None,
            },
            Vec::new(),
        ));
        local.upsert_interaction(BotInteractionSession {
            session_id: "i1".into(),
            conversation: conversation(),
            scope: InteractionScope::Conversation,
            actor_id: None,
            state_ref_id: "state".into(),
            wait: InteractionWaitSpec {
                event_kinds: Vec::new(),
                command: None,
                predicate_service_id: None,
                timeout_at_unix_ms: 1,
                propagation: BotPropagationPolicy::Continue,
                retry_prompt: None,
            },
            status: InteractionStatus::Waiting,
            generation: 1,
            version: 1,
            exclusive: true,
            retries_remaining: 0,
        });
        (
            QqBotManagementService::new(local.clone()),
            local,
        )
    }

    #[test]
    fn revision_fence_audit_and_secret_redaction() {
        let (api, _) = service();
        let open = api.snapshot("", true).unwrap();
        assert_eq!(open.revision, 0);
        assert_eq!(open.accounts[0].credential_status, "configured");
        let redacted = api.snapshot("", false).unwrap();
        assert_eq!(redacted.accounts[0].credential_reference, "");
        assert_eq!(redacted.accounts[0].credential_status, "restricted");

        let stale = api.write(QqManagementWriteRequest {
            actor_id: "op".into(),
            expected_revision: 9,
            action: QqManagementAction::AccountReconnect {
                account_id: "main".into(),
            },
        });
        assert_eq!(stale.unwrap_err().code, "revision.conflict");

        let written = api
            .write(QqManagementWriteRequest {
                actor_id: "op".into(),
                expected_revision: 0,
                action: QqManagementAction::AccountReconnect {
                    account_id: "main".into(),
                },
            })
            .unwrap();
        assert_eq!(written.revision, 1);
        assert_eq!(written.audit_id, "audit-1");
        assert_eq!(api.audits().len(), 1);
    }

    #[test]
    fn conversation_delivery_agent_and_interaction_writes() {
        let (api, _) = service();
        let mut policy = policy();
        policy.agent_enabled = false;
        let updated = api
            .write(QqManagementWriteRequest {
                actor_id: "op".into(),
                expected_revision: 0,
                action: QqManagementAction::ConversationUpdate {
                    conversation: conversation(),
                    policy,
                },
            })
            .unwrap();
        assert_eq!(updated.revision, 1);
        assert_eq!(updated.result["matched_rule_ids"][0], "console");

        api.write(QqManagementWriteRequest {
            actor_id: "op".into(),
            expected_revision: 1,
            action: QqManagementAction::DeliveryRetry {
                delivery_id: "d1".into(),
            },
        })
        .unwrap();
        api.write(QqManagementWriteRequest {
            actor_id: "op".into(),
            expected_revision: 2,
            action: QqManagementAction::AgentReset {
                origin_key: conversation().origin_key(),
            },
        })
        .unwrap();
        api.write(QqManagementWriteRequest {
            actor_id: "op".into(),
            expected_revision: 3,
            action: QqManagementAction::InteractionCancel {
                session_id: "i1".into(),
            },
        })
        .unwrap();
        let snap = api.snapshot("", true).unwrap();
        assert_eq!(snap.revision, 4);
        assert!(!snap.conversations[0].policy.agent_enabled);
        assert_eq!(snap.deliveries[0].receipt.status, DeliveryStatus::Pending);
        assert_eq!(snap.agent_sessions[0].status, "reset");
        assert_eq!(snap.interactions[0].status, InteractionStatus::Cancelled);
    }
}
