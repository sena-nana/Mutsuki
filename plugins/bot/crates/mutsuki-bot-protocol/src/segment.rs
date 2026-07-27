use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageSegment {
    Text {
        text: String,
    },
    MentionUser {
        user_id: String,
    },
    MentionAll,
    Image {
        resource: ResourceRef,
    },
    File {
        resource: ResourceRef,
        name: Option<String>,
    },
    Audio {
        resource: ResourceRef,
    },
    Video {
        resource: ResourceRef,
    },
    Reply {
        message_id: String,
    },
    Quote {
        message_id: String,
    },
    PlatformSpecific {
        platform: String,
        kind: String,
        payload: Value,
    },
}

impl MessageSegment {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}
