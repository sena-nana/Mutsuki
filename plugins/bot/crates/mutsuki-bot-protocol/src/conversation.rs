use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BotConversationKind, BotSpeechReplyPolicy, BotTarget};

pub const QQ_CONVERSATION_REF_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqConversationRef {
    pub version: u16,
    pub account_id: String,
    pub kind: BotConversationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

impl QqConversationRef {
    /// Validates the versioned identity before it is persisted or used as a target.
    pub fn validate(&self) -> Result<(), QqConversationRefError> {
        if self.version != QQ_CONVERSATION_REF_VERSION {
            return Err(QqConversationRefError::UnsupportedVersion(self.version));
        }
        if self.account_id.trim().is_empty() {
            return Err(QqConversationRefError::MissingField("account_id"));
        }
        for (field, value) in [
            ("account_id", self.account_id.as_str()),
            ("user_id", self.user_id.as_deref().unwrap_or_default()),
            ("group_id", self.group_id.as_deref().unwrap_or_default()),
            ("guild_id", self.guild_id.as_deref().unwrap_or_default()),
            ("channel_id", self.channel_id.as_deref().unwrap_or_default()),
            ("thread_id", self.thread_id.as_deref().unwrap_or_default()),
        ] {
            if value.contains('|') {
                return Err(QqConversationRefError::InvalidIdentifier { field });
            }
        }
        if self
            .thread_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(QqConversationRefError::MissingField("thread_id"));
        }
        match self.kind {
            BotConversationKind::Private => {
                require_non_empty(self.user_id.as_deref(), "user_id")?;
                reject_present(&self.group_id, "group_id")?;
                reject_present(&self.guild_id, "guild_id")?;
                reject_present(&self.channel_id, "channel_id")?;
            }
            BotConversationKind::Group => {
                require_non_empty(self.group_id.as_deref(), "group_id")?;
                reject_present(&self.user_id, "user_id")?;
                reject_present(&self.guild_id, "guild_id")?;
                reject_present(&self.channel_id, "channel_id")?;
            }
            BotConversationKind::Channel => {
                require_non_empty(self.guild_id.as_deref(), "guild_id")?;
                require_non_empty(self.channel_id.as_deref(), "channel_id")?;
                reject_present(&self.user_id, "user_id")?;
                reject_present(&self.group_id, "group_id")?;
            }
        }
        Ok(())
    }

    /// Reconstructs a v1 origin key so persisted mappings can be validated during migration.
    ///
    /// The key format is intentionally kept identical to `origin_key`; callers can validate
    /// and re-emit it without interpreting platform-specific business data.
    pub fn from_origin_key(origin_key: &str) -> Result<Self, QqConversationRefError> {
        let rest = origin_key
            .strip_prefix("qq:v")
            .ok_or(QqConversationRefError::InvalidOriginKey)?;
        let (version, encoded) = rest
            .split_once(':')
            .ok_or(QqConversationRefError::InvalidOriginKey)?;
        let version = version
            .parse::<u16>()
            .map_err(|_| QqConversationRefError::InvalidOriginKey)?;
        let parts = encoded
            .split('|')
            .map(parse_origin_part)
            .collect::<Result<Vec<_>, _>>()?;
        if parts.len() != 8 || parts[0] != version.to_string() {
            return Err(QqConversationRefError::InvalidOriginKey);
        }
        let kind = match parts[2].as_str() {
            "private" => BotConversationKind::Private,
            "group" => BotConversationKind::Group,
            "channel" => BotConversationKind::Channel,
            _ => return Err(QqConversationRefError::InvalidOriginKey),
        };
        let optional = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        let value = Self {
            version,
            account_id: parts[1].clone(),
            kind,
            user_id: optional(&parts[3]),
            group_id: optional(&parts[4]),
            guild_id: optional(&parts[5]),
            channel_id: optional(&parts[6]),
            thread_id: optional(&parts[7]),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn origin_key(&self) -> String {
        let parts = [
            self.version.to_string(),
            self.account_id.clone(),
            match self.kind {
                BotConversationKind::Private => "private",
                BotConversationKind::Group => "group",
                BotConversationKind::Channel => "channel",
            }
            .into(),
            self.user_id.clone().unwrap_or_default(),
            self.group_id.clone().unwrap_or_default(),
            self.guild_id.clone().unwrap_or_default(),
            self.channel_id.clone().unwrap_or_default(),
            self.thread_id.clone().unwrap_or_default(),
        ];
        let encoded = parts
            .iter()
            .map(|part| format!("{}:{part}", part.len()))
            .collect::<Vec<_>>()
            .join("|");
        format!("qq:v{}:{encoded}", self.version)
    }

    pub fn target(&self) -> Option<BotTarget> {
        self.validate().ok()?;
        match self.kind {
            BotConversationKind::Private => self.user_id.as_ref().map(|user_id| BotTarget::User {
                user_id: user_id.clone(),
            }),
            BotConversationKind::Group => self.group_id.as_ref().map(|group_id| BotTarget::Group {
                group_id: group_id.clone(),
            }),
            BotConversationKind::Channel => Some(BotTarget::GuildChannel {
                guild_id: self.guild_id.clone()?,
                channel_id: self.channel_id.clone()?,
            }),
        }
    }
}

fn require_non_empty(
    value: Option<&str>,
    field: &'static str,
) -> Result<(), QqConversationRefError> {
    if value.is_some_and(|value| !value.trim().is_empty()) {
        Ok(())
    } else {
        Err(QqConversationRefError::MissingField(field))
    }
}

fn reject_present(
    value: &Option<String>,
    field: &'static str,
) -> Result<(), QqConversationRefError> {
    if value.is_some() {
        Err(QqConversationRefError::UnexpectedField(field))
    } else {
        Ok(())
    }
}

fn parse_origin_part(part: &str) -> Result<String, QqConversationRefError> {
    let (length, value) = part
        .split_once(':')
        .ok_or(QqConversationRefError::InvalidOriginKey)?;
    let length = length
        .parse::<usize>()
        .map_err(|_| QqConversationRefError::InvalidOriginKey)?;
    if value.len() != length {
        return Err(QqConversationRefError::InvalidOriginKey);
    }
    Ok(value.to_owned())
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum QqConversationRefError {
    #[error("unsupported QQ conversation ref version: {0}")]
    UnsupportedVersion(u16),
    #[error("QQ conversation ref field is missing: {0}")]
    MissingField(&'static str),
    #[error("QQ conversation ref field is not valid for this kind: {0}")]
    UnexpectedField(&'static str),
    #[error("QQ conversation ref identifier contains an origin-key separator: {field}")]
    InvalidIdentifier { field: &'static str },
    #[error("QQ conversation origin key is invalid")]
    InvalidOriginKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionScope {
    #[default]
    SharedConversation,
    ActorInConversation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectMessagePolicy {
    #[default]
    Allow,
    Deny,
    Allowlisted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicy {
    pub revision: u64,
    pub enabled: bool,
    pub agent_enabled: bool,
    #[serde(default)]
    pub direct_message_policy: DirectMessagePolicy,
    pub must_mention: bool,
    #[serde(default)]
    pub wake_words: Vec<String>,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub denylist: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_profile_id: Option<String>,
    pub session_scope: AgentSessionScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub business_profile_binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_runtime_profile_id: Option<String>,
    pub stt_enabled: bool,
    pub tts_enabled: bool,
    #[serde(default)]
    pub speech_reply_policy: BotSpeechReplyPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_selector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_selector_id: Option<String>,
    pub active_delivery_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedConversationPolicy {
    pub conversation: QqConversationRef,
    pub policy: ConversationPolicy,
    pub matched_rule_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_rule_sources: Vec<ConversationPolicyRuleSource>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPolicyLayer {
    Account,
    Group,
    Guild,
    Channel,
    Conversation,
    ActorInConversation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicyRuleSource {
    pub rule_id: String,
    pub layer: ConversationPolicyLayer,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicyPatch {
    pub enabled: Option<bool>,
    pub agent_enabled: Option<bool>,
    pub direct_message_policy: Option<DirectMessagePolicy>,
    pub must_mention: Option<bool>,
    pub wake_words: Option<Vec<String>>,
    pub allowlist: Option<Vec<String>>,
    pub denylist: Option<Vec<String>>,
    pub rate_limit_profile_id: Option<Option<String>>,
    pub session_scope: Option<AgentSessionScope>,
    pub business_profile_binding_id: Option<Option<String>>,
    pub agent_runtime_profile_id: Option<Option<String>>,
    pub stt_enabled: Option<bool>,
    pub tts_enabled: Option<bool>,
    pub speech_reply_policy: Option<BotSpeechReplyPolicy>,
    pub stt_selector_id: Option<Option<String>>,
    pub tts_selector_id: Option<Option<String>>,
    pub active_delivery_enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicyMatch {
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BotConversationKind>,
    pub group_id: Option<String>,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub origin_key: Option<String>,
    pub actor_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicyRule {
    pub rule_id: String,
    pub revision: u64,
    pub matcher: ConversationPolicyMatch,
    pub patch: ConversationPolicyPatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionBinding {
    pub origin_key: String,
    pub session_id: String,
    pub session_version: u64,
    pub last_event_sequence: u64,
    pub policy_revision: u64,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BotAgentBridgeRequest {
    Submit {
        event: crate::BotEvent,
    },
    Cancel {
        event: crate::BotEvent,
        turn_id: String,
    },
    Reset {
        event: crate::BotEvent,
    },
    Fork {
        event: crate::BotEvent,
    },
    Status {
        event: crate::BotEvent,
    },
    Regenerate {
        event: crate::BotEvent,
    },
}
