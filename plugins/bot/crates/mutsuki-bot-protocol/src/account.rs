use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAccountRef {
    pub account_id: String,
    pub platform: BotPlatform,
}

/// Platforms the Bot contracts are shaped for.
///
/// Only `QqBot` has an adapter today. The rest are declared so the wire encoding of
/// a platform tag is fixed before a second adapter lands, the same way
/// [`crate::reserved`] pins protocol IDs; adding a variant later would change the
/// serialized form for every peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotPlatform {
    QqBot,
    OneBot,
    Discord,
    Telegram,
    Custom(String),
}

impl BotPlatform {
    pub fn as_str(&self) -> &str {
        match self {
            Self::QqBot => "qqbot",
            Self::OneBot => "onebot",
            Self::Discord => "discord",
            Self::Telegram => "telegram",
            Self::Custom(value) => value,
        }
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotAccountGetRequest {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotGatewayStatusRequest {}
