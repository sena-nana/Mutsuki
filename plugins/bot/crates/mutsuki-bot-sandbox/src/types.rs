use mutsuki_bot_protocol::{BotConversationKind, QqConversationRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SANDBOX_SERVICE_ID: &str = "mutsuki.bot.sandbox";
pub const DEFAULT_SANDBOX_ACCOUNT_ID: &str = "sandbox";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    Simulate,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxSpeakerRole {
    User,
    Bot,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxUserView {
    pub user_id: String,
    pub display_name: String,
    pub last_seen_unix_ms: u64,
    pub message_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxConversationView {
    pub conversation_id: String,
    pub account_id: String,
    pub kind: BotConversationKind,
    pub title: String,
    pub conversation: QqConversationRef,
    pub users: Vec<SandboxUserView>,
    pub last_preview: Option<String>,
    pub last_activity_unix_ms: u64,
    pub message_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMessageView {
    pub message_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub role: SandboxSpeakerRole,
    pub text: String,
    pub time_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxSnapshot {
    pub revision: u64,
    pub mode: SandboxMode,
    pub live_available: bool,
    pub account_id: String,
    pub conversations: Vec<SandboxConversationView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SandboxAction {
    SetMode {
        mode: SandboxMode,
    },
    IngestAsUser {
        conversation_id: String,
        user_id: String,
        text: String,
        #[serde(default)]
        inject_into_flow: bool,
    },
    SendAsBot {
        conversation_id: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxWriteRequest {
    #[serde(default)]
    pub operation_id: String,
    pub expected_revision: u64,
    pub action: SandboxAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxWriteResult {
    pub revision: u64,
    pub result: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxChangeEvent {
    pub revision: u64,
    pub mode: SandboxMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxError {
    pub code: String,
    pub message: String,
}

impl SandboxError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for SandboxError {}
