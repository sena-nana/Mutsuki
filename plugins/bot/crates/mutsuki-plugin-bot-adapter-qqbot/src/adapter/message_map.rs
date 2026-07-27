use mutsuki_bot_protocol::BotMessage;
use serde_json::Value;
use thiserror::Error;

use super::segment_map::SegmentMapError;
use crate::adapter::{qq_message_body_from_segments, qq_scene_and_openid};
use crate::api::SendMessagePayload;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MessageMapError {
    #[error("target is not supported by QQBot adapter")]
    UnsupportedTarget,
    #[error(transparent)]
    Segment(#[from] SegmentMapError),
}

pub fn bot_message_to_qq_send(message: BotMessage) -> Result<SendMessagePayload, MessageMapError> {
    let (scene, target_openid) =
        qq_scene_and_openid(&message.target).ok_or(MessageMapError::UnsupportedTarget)?;
    let mut body = qq_message_body_from_segments(&message.segments)?;
    if let (Value::Object(body), Some(reply_to)) = (&mut body, message.reply_to) {
        body.insert("msg_id".into(), Value::String(reply_to));
    }
    Ok(SendMessagePayload {
        scene,
        target_openid,
        body,
    })
}
