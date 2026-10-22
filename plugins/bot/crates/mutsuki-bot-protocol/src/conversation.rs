use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BotPlatform, BotSpeechReplyPolicy, BotTarget};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotConversationKind {
    Private,
    Group,
    Channel,
}

pub const BOT_CONVERSATION_REF_VERSION: u16 = 1;

/// Platform-neutral, versioned identity of one conversation on one bot account.
///
/// `origin_key` is the durable key every store uses; for QQ it keeps the historical `qq:v1:`
/// prefix so persisted keys never need rewriting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotConversationRef {
    pub version: u16,
    pub platform: BotPlatform,
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

impl BotConversationRef {
    /// Validates the versioned identity before it is persisted or used as a target.
    pub fn validate(&self) -> Result<(), BotConversationRefError> {
        if self.version != BOT_CONVERSATION_REF_VERSION {
            return Err(BotConversationRefError::UnsupportedVersion(self.version));
        }
        if self.account_id.trim().is_empty() {
            return Err(BotConversationRefError::MissingField("account_id"));
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
                return Err(BotConversationRefError::InvalidIdentifier { field });
            }
        }
        if self
            .thread_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(BotConversationRefError::MissingField("thread_id"));
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
    pub fn from_origin_key(origin_key: &str) -> Result<Self, BotConversationRefError> {
        let (prefix, rest) = origin_key
            .split_once(":v")
            .ok_or(BotConversationRefError::InvalidOriginKey)?;
        let platform = BotPlatform::from_origin_prefix(prefix)
            .ok_or(BotConversationRefError::InvalidOriginKey)?;
        let (version, encoded) = rest
            .split_once(':')
            .ok_or(BotConversationRefError::InvalidOriginKey)?;
        let version = version
            .parse::<u16>()
            .map_err(|_| BotConversationRefError::InvalidOriginKey)?;
        let parts = encoded
            .split('|')
            .map(parse_origin_part)
            .collect::<Result<Vec<_>, _>>()?;
        if parts.len() != 8 || parts[0] != version.to_string() {
            return Err(BotConversationRefError::InvalidOriginKey);
        }
        let kind = match parts[2].as_str() {
            "private" => BotConversationKind::Private,
            "group" => BotConversationKind::Group,
            "channel" => BotConversationKind::Channel,
            _ => return Err(BotConversationRefError::InvalidOriginKey),
        };
        let optional = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        let value = Self {
            version,
            platform,
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
        format!(
            "{}:v{}:{encoded}",
            self.platform.origin_prefix(),
            self.version
        )
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
) -> Result<(), BotConversationRefError> {
    if value.is_some_and(|value| !value.trim().is_empty()) {
        Ok(())
    } else {
        Err(BotConversationRefError::MissingField(field))
    }
}

fn reject_present(
    value: &Option<String>,
    field: &'static str,
) -> Result<(), BotConversationRefError> {
    if value.is_some() {
        Err(BotConversationRefError::UnexpectedField(field))
    } else {
        Ok(())
    }
}

fn parse_origin_part(part: &str) -> Result<String, BotConversationRefError> {
    let (length, value) = part
        .split_once(':')
        .ok_or(BotConversationRefError::InvalidOriginKey)?;
    let length = length
        .parse::<usize>()
        .map_err(|_| BotConversationRefError::InvalidOriginKey)?;
    if value.len() != length {
        return Err(BotConversationRefError::InvalidOriginKey);
    }
    Ok(value.to_owned())
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum BotConversationRefError {
    #[error("unsupported conversation ref version: {0}")]
    UnsupportedVersion(u16),
    #[error("conversation ref field is missing: {0}")]
    MissingField(&'static str),
    #[error("conversation ref field is not valid for this kind: {0}")]
    UnexpectedField(&'static str),
    #[error("conversation ref identifier contains an origin-key separator: {field}")]
    InvalidIdentifier { field: &'static str },
    #[error("conversation origin key is invalid")]
    InvalidOriginKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionScope {
    #[default]
    SharedConversation,
    ActorInConversation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicy {
    pub revision: u64,
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
    pub conversation: BotConversationRef,
    pub policy: ConversationPolicy,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn group(platform: BotPlatform) -> BotConversationRef {
        BotConversationRef {
            version: BOT_CONVERSATION_REF_VERSION,
            platform,
            account_id: "account-1".into(),
            kind: BotConversationKind::Group,
            user_id: None,
            group_id: Some("group-1".into()),
            guild_id: None,
            channel_id: None,
            thread_id: None,
        }
    }

    #[test]
    fn qq_origin_keys_keep_the_persisted_prefix() {
        let key = group(BotPlatform::QqBot).origin_key();
        assert_eq!(key, "qq:v1:1:1|9:account-1|5:group|0:|7:group-1|0:|0:|0:");
        assert_eq!(
            BotConversationRef::from_origin_key(&key).unwrap(),
            group(BotPlatform::QqBot)
        );
    }

    #[test]
    fn non_qq_platforms_round_trip_through_origin_keys_and_wire() {
        for platform in [
            BotPlatform::Sandbox,
            BotPlatform::OneBot,
            BotPlatform::Custom("matrix".into()),
        ] {
            let conversation = group(platform.clone());
            let key = conversation.origin_key();
            assert!(key.starts_with(&format!("{}:v1:", platform.origin_prefix())));
            assert_eq!(
                BotConversationRef::from_origin_key(&key).unwrap(),
                conversation
            );
            let wire = serde_json::to_value(&conversation).unwrap();
            assert_eq!(
                serde_json::from_value::<BotConversationRef>(wire).unwrap(),
                conversation
            );
        }
    }

    #[test]
    fn platform_wire_tag_matches_its_name() {
        for platform in [
            BotPlatform::QqBot,
            BotPlatform::OneBot,
            BotPlatform::Discord,
            BotPlatform::Telegram,
            BotPlatform::Sandbox,
        ] {
            assert_eq!(
                serde_json::to_value(&platform).unwrap(),
                serde_json::Value::String(platform.as_str().into())
            );
        }
    }
}
