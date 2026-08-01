use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};

use crate::{BotMediaUploadRequest, BotTarget};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotMediaTranscribeRequest {
    pub audio: ResourceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotMediaTranscribeResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotSpeechReplyPolicy {
    #[default]
    TextOnly,
    VoiceOnly,
    TextAndVoice,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotMediaSynthesizeRequest {
    pub target: BotTarget,
    pub text: String,
    pub policy: BotSpeechReplyPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_selector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub qq_mime_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotMediaSynthesizeResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<BotMediaUploadRequest>,
}
