use mutsuki_bot_protocol::{
    BotConversationKind, BotTarget, MessageSegment, QQ_CONVERSATION_REF_VERSION, QqConversationRef,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SANDBOX_SERVICE_ID: &str = "mutsuki.bot.sandbox";
pub const DEFAULT_SANDBOX_ACCOUNT_ID: &str = "sandbox";
pub const SANDBOX_ID_PREFIX: &str = "sandbox:";
pub const SANDBOX_GROUP_ID: &str = "sandbox:default";
pub const SANDBOX_USER_LIMIT: usize = 10;
pub const SANDBOX_USER_NAMES: [&str; SANDBOX_USER_LIMIT] = [
    "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Hank", "Ivy", "Jack",
];

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxMessageView {
    pub message_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub role: SandboxSpeakerRole,
    pub text: String,
    pub segments: Vec<MessageSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub time_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxSnapshot {
    pub revision: u64,
    pub mode: SandboxMode,
    pub live_available: bool,
    pub flow_available: bool,
    pub account_id: String,
    pub conversations: Vec<SandboxConversationView>,
    #[serde(default)]
    pub live_users: Vec<SandboxUserView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SandboxAction {
    SetMode {
        mode: SandboxMode,
    },
    AddUser,
    UpdateUser {
        user_id: String,
        new_user_id: String,
        display_name: String,
    },
    ImportLiveUsers {
        user_ids: Vec<String>,
    },
    RemoveUser {
        user_id: String,
    },
    ClearConversation {
        conversation_id: String,
    },
    IngestAsUser {
        conversation_id: String,
        user_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
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

#[must_use]
pub fn sandbox_user_id(display_name: &str) -> String {
    format!("{SANDBOX_ID_PREFIX}{}", display_name.to_ascii_lowercase())
}

#[must_use]
pub fn is_sandbox_id(value: &str) -> bool {
    value.starts_with(SANDBOX_ID_PREFIX)
}

#[must_use]
pub fn is_sandbox_conversation(conversation: &QqConversationRef) -> bool {
    conversation.user_id.as_deref().is_some_and(is_sandbox_id)
        || conversation.group_id.as_deref().is_some_and(is_sandbox_id)
        || conversation.guild_id.as_deref().is_some_and(is_sandbox_id)
        || conversation
            .channel_id
            .as_deref()
            .is_some_and(is_sandbox_id)
}

#[must_use]
pub fn is_sandbox_target(target: &BotTarget) -> bool {
    match target {
        BotTarget::User { user_id } => is_sandbox_id(user_id),
        BotTarget::Group { group_id } => is_sandbox_id(group_id),
        BotTarget::GuildChannel {
            guild_id,
            channel_id,
        } => is_sandbox_id(guild_id) || is_sandbox_id(channel_id),
        BotTarget::Conversation { conversation_id } => conversation_id.contains(SANDBOX_ID_PREFIX),
        BotTarget::PlatformSpecific { id, .. } => is_sandbox_id(id),
    }
}

#[must_use]
pub fn qq_conversation_from_target(
    account_id: impl Into<String>,
    target: &BotTarget,
) -> Option<QqConversationRef> {
    let (kind, user_id, group_id, guild_id, channel_id) = match target {
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
        BotTarget::Conversation { .. } | BotTarget::PlatformSpecific { .. } => return None,
    };
    let conversation = QqConversationRef {
        version: QQ_CONVERSATION_REF_VERSION,
        account_id: account_id.into(),
        kind,
        user_id,
        group_id,
        guild_id,
        channel_id,
        thread_id: None,
    };
    conversation.validate().ok()?;
    Some(conversation)
}

#[must_use]
pub fn preview_segments(segments: &[MessageSegment]) -> String {
    let mut parts = Vec::new();
    for segment in segments {
        match segment {
            MessageSegment::Text { text } => parts.push(text.clone()),
            MessageSegment::MentionUser { user_id } => {
                parts.push(format!("@{user_id}"));
            }
            MessageSegment::MentionAll => parts.push("@全体成员".into()),
            MessageSegment::Image { .. } => parts.push("[图片]".into()),
            MessageSegment::File { name, .. } => {
                parts.push(format!("[{}]", name.as_deref().unwrap_or("文件")));
            }
            MessageSegment::Audio { .. } => parts.push("[语音]".into()),
            MessageSegment::Video { .. } => parts.push("[视频]".into()),
            MessageSegment::Reply { .. } | MessageSegment::Quote { .. } => {}
            MessageSegment::PlatformSpecific { kind, .. } => {
                parts.push(format!("[{kind}]"));
            }
        }
    }
    parts.join("")
}
