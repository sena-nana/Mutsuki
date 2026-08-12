use serde::{Deserialize, Serialize};

use crate::{BotEventKind, BotMessage, QqConversationRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionScope {
    Conversation,
    ActorInConversation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InteractionWaitSpec {
    #[serde(default)]
    pub event_kinds: Vec<BotEventKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate_service_id: Option<String>,
    pub timeout_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_prompt: Option<BotMessage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    Waiting,
    Completed,
    TimedOut,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotInteractionSession {
    pub session_id: String,
    pub conversation: QqConversationRef,
    pub scope: InteractionScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    pub state_ref_id: String,
    pub wait: InteractionWaitSpec,
    pub status: InteractionStatus,
    pub generation: u64,
    pub version: u64,
    pub exclusive: bool,
    pub retries_remaining: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InteractionMatch {
    pub session_id: String,
    pub event_id: String,
    pub next_version: u64,
    pub accepted: bool,
    pub status: InteractionStatus,
    pub state_ref_id: String,
    pub retries_remaining: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_prompt: Option<BotMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionResult {
    pub session_id: String,
    pub status: InteractionStatus,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BotInteractionCommand {
    Create {
        session: BotInteractionSession,
    },
    MatchEvent {
        event: crate::BotEvent,
        now_unix_ms: u64,
    },
    Cancel {
        session: BotInteractionSession,
    },
    Transition {
        session: BotInteractionSession,
        next_state_ref_id: String,
        next_wait: InteractionWaitSpec,
        retries_remaining: u32,
    },
    Recover {
        now_unix_ms: u64,
    },
    RecoverGeneration {
        now_unix_ms: u64,
        active_generation: u64,
    },
}
