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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqStreamingStrategy {
    FinalOnly,
    SegmentMessages,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotCapabilityMatrix {
    pub account_id: String,
    pub conversation_kinds: Vec<BotConversationKind>,
    pub outbound_conversation_kinds: Vec<BotConversationKind>,
    pub active_message_kinds: Vec<BotConversationKind>,
    pub inbound_media: Vec<BotMediaKind>,
    pub outbound_media: Vec<BotMediaKind>,
    pub active_message: bool,
    pub message_edit: bool,
    pub message_recall: bool,
    pub reply: bool,
    pub mention: bool,
    pub upload: QqUploadConstraints,
    pub streaming: Vec<QqStreamingStrategy>,
    pub required_intents: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotCapabilityGetRequest {}
