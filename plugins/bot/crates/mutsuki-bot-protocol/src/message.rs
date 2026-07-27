use serde::{Deserialize, Serialize};

use crate::{BotExtMap, BotTarget, BotUser, MessageSegment};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotMessage {
    pub message_id: Option<String>,
    pub target: BotTarget,
    pub sender: Option<BotUser>,
    pub segments: Vec<MessageSegment>,
    pub reply_to: Option<String>,
    pub time_ms: Option<i64>,
    pub ext: BotExtMap,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotMessageRecallRequest {
    pub target: BotTarget,
    pub message_id: String,
}

impl BotMessage {
    pub fn text(target: BotTarget, text: impl Into<String>) -> Self {
        Self {
            message_id: None,
            target,
            sender: None,
            segments: vec![MessageSegment::text(text)],
            reply_to: None,
            time_ms: None,
            ext: BotExtMap::new(),
        }
    }

    pub fn plain_text(&self) -> String {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                MessageSegment::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
