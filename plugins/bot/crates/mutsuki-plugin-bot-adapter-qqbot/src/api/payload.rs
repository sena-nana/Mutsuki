use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqScene {
    Group,
    C2c,
}

impl QqScene {
    pub fn messages_path(&self, target_openid: &str) -> String {
        match self {
            Self::Group => format!("/v2/groups/{target_openid}/messages"),
            Self::C2c => format!("/v2/users/{target_openid}/messages"),
        }
    }

    pub fn files_path(&self, target_openid: &str) -> String {
        match self {
            Self::Group => format!("/v2/groups/{target_openid}/files"),
            Self::C2c => format!("/v2/users/{target_openid}/files"),
        }
    }

    pub fn upload_prepare_path(&self, target_openid: &str) -> String {
        match self {
            Self::Group => format!("/v2/groups/{target_openid}/upload_prepare"),
            Self::C2c => format!("/v2/users/{target_openid}/upload_prepare"),
        }
    }

    pub fn upload_part_finish_path(&self, target_openid: &str) -> String {
        match self {
            Self::Group => format!("/v2/groups/{target_openid}/upload_part_finish"),
            Self::C2c => format!("/v2/users/{target_openid}/upload_part_finish"),
        }
    }

    pub fn recall_path(&self, target_openid: &str, message_id: &str) -> String {
        match self {
            Self::Group => format!("/v2/groups/{target_openid}/messages/{message_id}"),
            Self::C2c => format!("/v2/users/{target_openid}/messages/{message_id}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendMessagePayload {
    pub scene: QqScene,
    pub target_openid: String,
    pub body: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaUploadPayload {
    pub scene: QqScene,
    pub target_openid: String,
    pub file_type: u8,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub file_data: Option<String>,
    #[serde(default)]
    pub resource_ref: Option<ResourceRef>,
    #[serde(default)]
    pub upload_id: Option<String>,
    #[serde(default)]
    pub srv_send_msg: Option<bool>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub md5: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub md5_10m: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecallMessagePayload {
    pub scene: QqScene,
    pub target_openid: String,
    pub message_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawCallPayload {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Option<Value>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PayloadError {
    #[error("payload must be a JSON object")]
    NotObject,
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
    #[error("group messages do not support field: {0}")]
    UnsupportedGroupField(&'static str),
    #[error("exactly one media upload source is required")]
    InvalidMediaSource,
    #[error("stream.index must be 0 for the first C2C stream fragment")]
    InvalidStreamIndex,
}

pub fn parse_payload<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, PayloadError> {
    if !value.is_object() {
        return Err(PayloadError::NotObject);
    }
    serde_json::from_value(value).map_err(|_| PayloadError::InvalidField("payload"))
}

impl SendMessagePayload {
    pub fn validated_body(&self) -> Result<Value, PayloadError> {
        let body = self.body.as_object().ok_or(PayloadError::NotObject)?;
        if !body.contains_key("msg_type") {
            return Err(PayloadError::MissingField("msg_type"));
        }
        if self.target_openid.trim().is_empty() {
            return Err(PayloadError::MissingField("target_openid"));
        }
        if self.scene == QqScene::Group {
            for field in ["stream", "prompt_keyboard", "action_button"] {
                if body.contains_key(field) {
                    return Err(PayloadError::UnsupportedGroupField(field));
                }
            }
        }
        if self.scene == QqScene::C2c {
            validate_c2c_stream(body)?;
        }
        Ok(self.body.clone())
    }
}

impl MediaUploadPayload {
    pub fn validate(&self) -> Result<(), PayloadError> {
        if self.target_openid.trim().is_empty() {
            return Err(PayloadError::MissingField("target_openid"));
        }
        if !(1..=4).contains(&self.file_type) {
            return Err(PayloadError::InvalidField("file_type"));
        }
        let source_count = [
            self.url.is_some(),
            self.file_data.is_some(),
            self.resource_ref.is_some(),
            self.upload_id.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if source_count != 1 {
            return Err(PayloadError::InvalidMediaSource);
        }
        Ok(())
    }
}

fn validate_c2c_stream(body: &serde_json::Map<String, Value>) -> Result<(), PayloadError> {
    let Some(stream) = body.get("stream") else {
        return Ok(());
    };
    let stream = stream
        .as_object()
        .ok_or(PayloadError::InvalidField("stream"))?;
    let index = stream
        .get("index")
        .and_then(Value::as_u64)
        .ok_or(PayloadError::InvalidField("stream.index"))?;
    if !stream.contains_key("id") && index != 0 {
        return Err(PayloadError::InvalidStreamIndex);
    }
    Ok(())
}
