use serde::{Deserialize, Serialize};

use crate::{BotEvent, BotEventKind, BotPermission};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotConversationKind {
    Private,
    Group,
    Channel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotActorRole {
    Member,
    Administrator,
    Owner,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotFilterExpr {
    All { filters: Vec<Self> },
    Any { filters: Vec<Self> },
    Not { filter: Box<Self> },
    ConversationKind { kind: BotConversationKind },
    EventKind { kind: BotEventKind },
    MustMentionBot,
    IsReply,
    ActorRole { role: BotActorRole },
    Account { account_id: String },
    ActorAllowlist { actor_ids: Vec<String> },
    ActorDenylist { actor_ids: Vec<String> },
    MessageSegmentType { segment_type: String },
    CustomPredicate { service_id: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotPropagationPolicy {
    #[default]
    Continue,
    StopOnSuccess,
    ConsumeOnSuccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotCooldownScope {
    Actor,
    Conversation,
    Command,
    ActorAndConversation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRateLimitPolicy {
    pub scope: BotCooldownScope,
    pub capacity: u32,
    pub refill_tokens: u32,
    pub refill_interval_ms: u64,
    #[serde(default)]
    pub administrator_bypass: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotHandlerDescriptor {
    pub handler_id: String,
    pub binding_id: String,
    pub generation: u64,
    pub handler_protocol_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_hint: Option<String>,
    #[serde(default)]
    pub event_kinds: Vec<BotEventKind>,
    #[serde(default)]
    pub conversation_kinds: Vec<BotConversationKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<BotFilterExpr>,
    #[serde(default)]
    pub permissions: Vec<BotPermission>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub propagation: BotPropagationPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<BotRateLimitPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub side_effects: Vec<String>,
    #[serde(default)]
    pub max_concurrency: Option<u32>,
    #[serde(default)]
    pub before_hook_protocol_ids: Vec<String>,
    #[serde(default)]
    pub after_hook_protocol_ids: Vec<String>,
    #[serde(default)]
    pub error_hook_protocol_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotHandlerHookRequest {
    pub handler_id: String,
    pub phase: String,
    pub event: BotEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotPermissionCheckRequest {
    pub handler: BotHandlerDescriptor,
    pub event: BotEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotPermissionCheckResult {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotRateLimitCheckRequest {
    pub handler_id: String,
    pub policy: BotRateLimitPolicy,
    pub event: BotEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRateLimitCheckResult {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotCustomPredicateRequest {
    pub event: BotEvent,
    pub handler_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCustomPredicateResult {
    pub matched: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotHandlerOutcome {
    Continue,
    Stop,
    Consume,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotHandlerExecutionResult {
    pub outcome: BotHandlerOutcome,
}
