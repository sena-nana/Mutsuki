use mutsuki_bot_protocol::{
    BotConversationKind, BotTarget, BotUser, MessageSegment, QQ_CONVERSATION_REF_VERSION,
    QqConversationRef,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::content::SandboxContentRef;

pub const SANDBOX_SERVICE_ID: &str = "mutsuki.bot.sandbox";
pub const DEFAULT_SANDBOX_ACCOUNT_ID: &str = "sandbox";
pub const SANDBOX_ID_PREFIX: &str = "sandbox:";
pub const SANDBOX_GROUP_ID: &str = "sandbox:default";
pub const SANDBOX_USER_LIMIT: usize = 10;
pub const SANDBOX_USER_NAMES: [&str; SANDBOX_USER_LIMIT] = [
    "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Hank", "Ivy", "Jack",
];
pub const SANDBOX_MAX_MESSAGES: usize = 200;
pub const SANDBOX_MAX_MEDIA_ITEMS: usize = 20;
pub const SANDBOX_MAX_MEDIA_BYTES: usize = 2 * 1024 * 1024;
pub const SANDBOX_MAX_STICKER_ITEMS: usize = 50;
pub const SANDBOX_MAX_STICKER_BYTES: usize = SANDBOX_MAX_MEDIA_BYTES;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    #[default]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxHistoryKind {
    Simulate,
    Live,
}

impl SandboxHistoryKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simulate => "simulate",
            Self::Live => "live",
        }
    }

    /// Parses a persisted store kind.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not `simulate` or `live`.
    pub fn parse(value: &str) -> Result<Self, SandboxError> {
        match value {
            "simulate" => Ok(Self::Simulate),
            "live" => Ok(Self::Live),
            _ => Err(SandboxError::new(
                "sandbox.history",
                format!("unknown sandbox history kind `{value}`"),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxHistoryConversation {
    pub view: SandboxConversationView,
    pub users: Vec<SandboxUserView>,
    pub messages: Vec<SandboxMessageView>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SandboxHistorySnapshot {
    pub mode: SandboxMode,
    #[serde(default)]
    pub account_id: String,
    pub simulate: Vec<SandboxHistoryConversation>,
    pub live: Vec<SandboxHistoryConversation>,
    pub media: Vec<SandboxAsset>,
    #[serde(default)]
    pub stickers: Vec<SandboxSticker>,
    #[serde(default)]
    pub faces: Vec<SandboxFace>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxUserView {
    pub user_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub last_seen_unix_ms: u64,
    pub message_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxConversationView {
    pub conversation_id: String,
    pub account_id: String,
    pub kind: BotConversationKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub conversation: QqConversationRef,
    pub users: Vec<SandboxUserView>,
    pub last_preview: Option<String>,
    pub last_activity_unix_ms: u64,
    pub message_count: u64,
    #[serde(default)]
    pub active_message: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxMessageView {
    pub message_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub role: SandboxSpeakerRole,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<SandboxContentRef>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<BotUser>,
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
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        segments: Vec<MessageSegment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
    },
    SendAsBot {
        conversation_id: String,
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        segments: Vec<MessageSegment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMediaRef {
    pub media_id: String,
    pub mime: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMediaBlob {
    pub media_id: String,
    pub mime: String,
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxAsset {
    pub content_hash: String,
    pub kind: String,
    pub mime: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub created_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSticker {
    pub content_hash: String,
    pub mime: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bytes: Vec<u8>,
    pub created_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxFace {
    pub face_key: String,
    pub face_type: String,
    pub face_id: String,
    pub last_seen_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStickerKind {
    Custom,
    QqFace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxStickerView {
    pub id: String,
    pub kind: SandboxStickerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_id: Option<String>,
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
            MessageSegment::Markdown { .. } => parts.push("[Markdown]".into()),
            MessageSegment::PlatformSpecific { kind, payload, .. } => {
                parts.push(preview_platform_kind(kind, payload));
            }
        }
    }
    parts.join("")
}

fn preview_platform_kind(kind: &str, payload: &Value) -> String {
    match kind {
        "ark" | "embed" => "[小卡片]".into(),
        "markdown" => "[Markdown]".into(),
        "keyboard" => "[按钮]".into(),
        "attachment" | "media" => preview_media_label(payload),
        "sticker" => "[表情包]".into(),
        "face" => "[表情]".into(),
        other => format!("[{other}]"),
    }
}

fn preview_media_label(payload: &Value) -> String {
    let mime = payload
        .get("content_type")
        .or_else(|| payload.get("mime"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mime.starts_with("image/") {
        "[图片]".into()
    } else if mime.starts_with("audio/") {
        "[语音]".into()
    } else if mime.starts_with("video/") {
        "[视频]".into()
    } else {
        let name = payload
            .get("filename")
            .or_else(|| payload.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("文件");
        format!("[{name}]")
    }
}

#[must_use]
pub fn parse_sandbox_mentions(text: &str, users: &[SandboxUserView]) -> Vec<MessageSegment> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        match next_sandbox_mention(&text[cursor..], users) {
            None => {
                let rest = &text[cursor..];
                if !rest.is_empty() {
                    segments.push(MessageSegment::text(rest));
                }
                break;
            }
            Some((start, end, segment)) => {
                if start > 0 {
                    segments.push(MessageSegment::text(&text[cursor..cursor + start]));
                }
                segments.push(segment);
                cursor += end;
            }
        }
    }
    segments
}

fn next_sandbox_mention(
    text: &str,
    users: &[SandboxUserView],
) -> Option<(usize, usize, MessageSegment)> {
    let mut best: Option<(usize, usize, MessageSegment)> = None;
    consider_mention(
        &mut best,
        text.find("@全体成员")
            .map(|start| (start, start + "@全体成员".len(), MessageSegment::MentionAll)),
    );
    let mut search = 0;
    while let Some(rel) = text[search..].find("<@") {
        let start = search + rel;
        if let Some(body) = text[start..].strip_prefix("<@")
            && let Some((user_id, _)) = body.split_once('>')
            && !user_id.is_empty()
            && !user_id.chars().any(char::is_whitespace)
        {
            let end = start + 2 + user_id.len() + 1;
            consider_mention(
                &mut best,
                Some((
                    start,
                    end,
                    MessageSegment::MentionUser {
                        user_id: user_id.to_owned(),
                    },
                )),
            );
            break;
        }
        search = start + 2;
    }
    let mut names = users
        .iter()
        .flat_map(|user| {
            [
                (user.display_name.as_str(), user.user_id.as_str()),
                (user.user_id.as_str(), user.user_id.as_str()),
            ]
        })
        .filter(|(name, _)| !name.is_empty())
        .collect::<Vec<_>>();
    names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    for (name, user_id) in names {
        let needle = format!("@{name}");
        if let Some(start) = text.find(&needle) {
            consider_mention(
                &mut best,
                Some((
                    start,
                    start + needle.len(),
                    MessageSegment::MentionUser {
                        user_id: user_id.to_owned(),
                    },
                )),
            );
        }
    }
    best
}

fn consider_mention(
    best: &mut Option<(usize, usize, MessageSegment)>,
    hit: Option<(usize, usize, MessageSegment)>,
) {
    let Some((start, end, segment)) = hit else {
        return;
    };
    match best {
        None => *best = Some((start, end, segment)),
        Some((current_start, current_end, _))
            if start < *current_start || (start == *current_start && end > *current_end) =>
        {
            *best = Some((start, end, segment));
        }
        _ => {}
    }
}
