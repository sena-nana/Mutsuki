use mutsuki_bot_protocol::{BotMediaKind, BotMediaUploadRequest, BotMessageRecallRequest};
use thiserror::Error;

use crate::adapter::qq_scene_and_openid;
use crate::api::{MediaUploadPayload, RecallMessagePayload};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MediaMapError {
    #[error("target is not supported by QQBot adapter")]
    UnsupportedTarget,
}

pub fn bot_media_upload_to_qq_upload(
    request: BotMediaUploadRequest,
) -> Result<MediaUploadPayload, MediaMapError> {
    let (scene, target_openid) =
        qq_scene_and_openid(&request.target).ok_or(MediaMapError::UnsupportedTarget)?;
    let file_size = request.resource.size_hint;
    Ok(MediaUploadPayload {
        scene,
        target_openid,
        file_type: qq_file_type(request.kind),
        url: None,
        file_data: None,
        resource_ref: Some(request.resource),
        upload_id: None,
        srv_send_msg: None,
        file_name: request.file_name,
        file_size,
        md5: None,
        sha1: None,
        md5_10m: None,
    })
}

pub fn bot_recall_to_qq_recall(
    request: BotMessageRecallRequest,
) -> Result<RecallMessagePayload, MediaMapError> {
    let (scene, target_openid) =
        qq_scene_and_openid(&request.target).ok_or(MediaMapError::UnsupportedTarget)?;
    Ok(RecallMessagePayload {
        scene,
        target_openid,
        message_id: request.message_id,
    })
}

fn qq_file_type(kind: BotMediaKind) -> u8 {
    match kind {
        BotMediaKind::Image => 1,
        BotMediaKind::Video => 2,
        BotMediaKind::Audio => 3,
        BotMediaKind::File => 4,
    }
}
