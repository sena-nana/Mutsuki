use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BotConversationKind, BotMediaKind};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotUploadConstraints {
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub max_bytes_by_kind: BTreeMap<BotMediaKind, u64>,
    pub allowed_mime_types: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRateLimitPolicy {
    /// The platform owns the effective request quota and communicates it through its responses.
    pub server_driven: bool,
    pub honors_retry_after: bool,
    pub max_retry_attempts: u8,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub gateway_rate_limit_delay_ms: u64,
}

/// How an adapter can surface a long Agent reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotStreamingStrategy {
    FinalOnly,
    SegmentMessages,
}

/// Message segment kinds an adapter accepts or produces. `Keyboard` covers platform action rows
/// (QQ keyboards, Telegram inline keyboards); their payload stays `PlatformSpecific`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotSegmentKind {
    Text,
    MentionUser,
    MentionAll,
    Image,
    Audio,
    Video,
    File,
    Reply,
    Quote,
    Markdown,
    Keyboard,
}

/// Platform-neutral, account-scoped capabilities of one adapter account. It is the source of
/// truth for what a plugin may compose; platform-only facts (intents, permissions) stay in the
/// platform's own capability type that embeds this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAdapterCapabilities {
    pub conversation_kinds: Vec<BotConversationKind>,
    pub outbound_conversation_kinds: Vec<BotConversationKind>,
    pub active_message_kinds: Vec<BotConversationKind>,
    #[serde(default)]
    pub inbound_segments: Vec<BotSegmentKind>,
    #[serde(default)]
    pub outbound_segments: Vec<BotSegmentKind>,
    pub inbound_media: Vec<BotMediaKind>,
    pub outbound_media: Vec<BotMediaKind>,
    pub active_message: bool,
    pub message_edit: bool,
    pub message_recall: bool,
    pub reply: bool,
    #[serde(default)]
    pub quote: bool,
    pub mention: bool,
    pub upload: BotUploadConstraints,
    #[serde(default)]
    pub rate_limit: BotRateLimitPolicy,
    pub streaming: Vec<BotStreamingStrategy>,
}
