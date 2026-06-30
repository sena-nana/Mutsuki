use mutsuki_bot_protocol::BotMessage;
use thiserror::Error;

use crate::adapter::{qq_message_body_from_segments, qq_scene_and_openid};
use crate::api::SendMessagePayload;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MessageMapError {
    #[error("target is not supported by QQBot adapter")]
    UnsupportedTarget,
}

pub fn bot_message_to_qq_send(message: BotMessage) -> Result<SendMessagePayload, MessageMapError> {
    let (scene, target_openid) =
        qq_scene_and_openid(&message.target).ok_or(MessageMapError::UnsupportedTarget)?;
    Ok(SendMessagePayload {
        scene,
        target_openid,
        body: qq_message_body_from_segments(&message.segments),
    })
}
