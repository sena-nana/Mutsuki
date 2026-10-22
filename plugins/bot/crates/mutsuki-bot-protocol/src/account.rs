use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAccountRef {
    pub account_id: String,
    pub platform: BotPlatform,
}

/// Platforms the Bot contracts are shaped for.
///
/// `QqBot` has a production adapter and `Sandbox` is the simulated adapter. The rest are declared
/// so the wire encoding of a platform tag is fixed before their adapters land, the same way
/// [`crate::reserved`] pins protocol IDs. The serialized tag always equals [`Self::as_str`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotPlatform {
    #[serde(rename = "qqbot")]
    QqBot,
    #[serde(rename = "onebot")]
    OneBot,
    Discord,
    Telegram,
    Sandbox,
    Custom(String),
}

impl BotPlatform {
    pub fn as_str(&self) -> &str {
        match self {
            Self::QqBot => "qqbot",
            Self::OneBot => "onebot",
            Self::Discord => "discord",
            Self::Telegram => "telegram",
            Self::Sandbox => "sandbox",
            Self::Custom(value) => value,
        }
    }

    /// Prefix of [`crate::BotConversationRef::origin_key`]. QQ keeps its historical `qq`.
    pub fn origin_prefix(&self) -> &str {
        match self {
            Self::QqBot => "qq",
            other => other.as_str(),
        }
    }

    pub fn from_origin_prefix(prefix: &str) -> Option<Self> {
        if prefix.is_empty() || prefix.contains(':') {
            return None;
        }
        Some(match prefix {
            "qq" => Self::QqBot,
            "onebot" => Self::OneBot,
            "discord" => Self::Discord,
            "telegram" => Self::Telegram,
            "sandbox" => Self::Sandbox,
            other => Self::Custom(other.to_owned()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotUser {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Guild/channel shaped platforms (Discord and friends) address conversations through
/// these; no first-party adapter produces them yet. Kept with [`BotPlatform`] so the
/// neutral account model does not have to change shape when one arrives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotGuild {
    pub guild_id: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChannel {
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub name: Option<String>,
}
