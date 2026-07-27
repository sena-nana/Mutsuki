use mutsuki_bot_protocol::MessageSegment;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SegmentMapError {
    #[error("message segment is not supported by QQBot standard send: {0}")]
    UnsupportedSegment(&'static str),
}

pub fn qq_message_body_from_segments(
    segments: &[MessageSegment],
) -> Result<Value, SegmentMapError> {
    let mut content = String::new();
    for segment in segments {
        match segment {
            MessageSegment::Text { text } => content.push_str(text),
            MessageSegment::MentionUser { user_id } => {
                content.push_str(&format!("<@{user_id}>"));
            }
            MessageSegment::MentionAll => content.push_str("@all"),
            unsupported => {
                return Err(SegmentMapError::UnsupportedSegment(segment_name(
                    unsupported,
                )));
            }
        }
    }
    Ok(json!({
        "msg_type": 0,
        "content": content,
    }))
}

fn segment_name(segment: &MessageSegment) -> &'static str {
    match segment {
        MessageSegment::Text { .. } => "text",
        MessageSegment::MentionUser { .. } => "mention_user",
        MessageSegment::MentionAll => "mention_all",
        MessageSegment::Image { .. } => "image",
        MessageSegment::File { .. } => "file",
        MessageSegment::Audio { .. } => "audio",
        MessageSegment::Video { .. } => "video",
        MessageSegment::Reply { .. } => "reply",
        MessageSegment::Quote { .. } => "quote",
        MessageSegment::PlatformSpecific { .. } => "platform_specific",
    }
}
