use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotTarget {
    User {
        user_id: String,
    },
    Group {
        group_id: String,
    },
    GuildChannel {
        guild_id: String,
        channel_id: String,
    },
    Conversation {
        conversation_id: String,
    },
    PlatformSpecific {
        platform: String,
        kind: String,
        id: String,
    },
}

impl BotTarget {
    pub fn platform_specific(
        platform: impl Into<String>,
        kind: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self::PlatformSpecific {
            platform: platform.into(),
            kind: kind.into(),
            id: id.into(),
        }
    }
}
