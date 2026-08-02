use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BotConversationKind, BotMediaKind};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqUploadConstraints {
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub max_bytes_by_kind: BTreeMap<BotMediaKind, u64>,
    pub allowed_mime_types: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqRateLimitPolicy {
    /// QQ owns the effective request quota and communicates it through HTTP responses.
    pub server_driven: bool,
    pub honors_retry_after: bool,
    pub max_retry_attempts: u8,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub gateway_rate_limit_delay_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqStreamingStrategy {
    FinalOnly,
    SegmentMessages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqMessageSegmentKind {
    Text,
    MentionUser,
    MentionAll,
    Image,
    Audio,
    Video,
    File,
    Reply,
    Quote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqPermissionRequirement {
    ReadGroupMessages,
    ReadC2cMessages,
    ReadGuildAtMessages,
    SendGroupMessages,
    SendC2cMessages,
    UploadGroupMedia,
    UploadC2cMedia,
    RecallGroupMessages,
    RecallC2cMessages,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotCapabilityMatrix {
    pub account_id: String,
    pub conversation_kinds: Vec<BotConversationKind>,
    pub outbound_conversation_kinds: Vec<BotConversationKind>,
    pub active_message_kinds: Vec<BotConversationKind>,
    #[serde(default)]
    pub inbound_segments: Vec<QqMessageSegmentKind>,
    #[serde(default)]
    pub outbound_segments: Vec<QqMessageSegmentKind>,
    pub inbound_media: Vec<BotMediaKind>,
    pub outbound_media: Vec<BotMediaKind>,
    pub active_message: bool,
    pub message_edit: bool,
    pub message_recall: bool,
    pub reply: bool,
    #[serde(default)]
    pub quote: bool,
    pub mention: bool,
    pub upload: QqUploadConstraints,
    #[serde(default)]
    pub rate_limit: QqRateLimitPolicy,
    pub streaming: Vec<QqStreamingStrategy>,
    #[serde(default)]
    pub configured_intents: u64,
    #[serde(default)]
    pub shard: [u64; 2],
    pub required_intents: Vec<String>,
    #[serde(default)]
    pub required_permissions: Vec<QqPermissionRequirement>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotCapabilityGetRequest {}
