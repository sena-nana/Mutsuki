use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::api::{
    HttpMethod, QqAuthManager, QqBotClients, QqHttpRequest, QqIdSource, QqOpenApiError,
    authorization_header, json_field, request_json,
};
use crate::config::QqBotConfig;

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
    pub resource_ref: Option<String>,
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

pub struct QqOpenApiService {
    config: QqBotConfig,
    auth: QqAuthManager,
    clients: QqBotClients,
    id_source: Box<dyn QqIdSource>,
}

impl QqOpenApiService {
    pub fn new(config: QqBotConfig, clients: QqBotClients, id_source: Box<dyn QqIdSource>) -> Self {
        Self {
            config,
            auth: QqAuthManager::new(),
            clients,
            id_source,
        }
    }

    pub fn send_message(
        &mut self,
        payload: SendMessagePayload,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let mut body = payload
            .validated_body()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        ensure_msg_seq(&mut body, self.id_source.next_msg_seq());
        self.execute_openapi_json(
            HttpMethod::Post,
            payload.scene.messages_path(&payload.target_openid),
            body,
            current_step,
        )
    }

    pub fn upload_media(
        &mut self,
        payload: MediaUploadPayload,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        payload
            .validate()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        if let Some(upload_id) = &payload.upload_id {
            return self.exchange_upload_id(&payload, upload_id, current_step);
        }
        if payload.resource_ref.is_some() {
            return self.upload_resource_chunks(payload, current_step);
        }
        let mut body = json!({
            "file_type": payload.file_type,
            "srv_send_msg": payload.srv_send_msg.unwrap_or(false),
        });
        insert_optional(&mut body, "url", payload.url);
        insert_optional(&mut body, "file_data", payload.file_data);
        insert_optional(&mut body, "file_name", payload.file_name);
        let response = self.execute_openapi_json(
            HttpMethod::Post,
            payload.scene.files_path(&payload.target_openid),
            body,
            current_step,
        )?;
        ensure_file_info(&response)?;
        Ok(response)
    }

    pub fn recall_message(
        &mut self,
        payload: RecallMessagePayload,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        if payload.target_openid.trim().is_empty() || payload.message_id.trim().is_empty() {
            return Err(QqOpenApiError::InvalidPayload(
                "target_openid and message_id are required".into(),
            ));
        }
        self.execute_openapi_json(
            HttpMethod::Delete,
            payload
                .scene
                .recall_path(&payload.target_openid, &payload.message_id),
            Value::Null,
            current_step,
        )
    }

    pub fn raw_call(
        &mut self,
        payload: RawCallPayload,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let method = match payload.method.as_str() {
            "POST" | "post" => HttpMethod::Post,
            "PUT" | "put" => HttpMethod::Put,
            "DELETE" | "delete" => HttpMethod::Delete,
            _ => return Err(QqOpenApiError::InvalidPayload("unsupported method".into())),
        };
        self.execute_openapi_json(
            method,
            payload.path,
            payload.body.unwrap_or(Value::Null),
            current_step,
        )
    }

    fn exchange_upload_id(
        &mut self,
        payload: &MediaUploadPayload,
        upload_id: &str,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let response = self.execute_openapi_json(
            HttpMethod::Post,
            payload.scene.files_path(&payload.target_openid),
            json!({ "upload_id": upload_id }),
            current_step,
        )?;
        ensure_file_info(&response)?;
        Ok(response)
    }

    fn upload_resource_chunks(
        &mut self,
        payload: MediaUploadPayload,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let resource_ref = payload
            .resource_ref
            .clone()
            .ok_or_else(|| QqOpenApiError::InvalidPayload("resource_ref is required".into()))?;
        let prepare = self.execute_openapi_json(
            HttpMethod::Post,
            payload.scene.upload_prepare_path(&payload.target_openid),
            json!({
                "file_type": payload.file_type,
                "file_name": payload.file_name.clone().unwrap_or_else(|| "media.bin".into()),
                "file_size": payload.file_size.unwrap_or(0),
                "md5": payload.md5.clone().unwrap_or_default(),
                "sha1": payload.sha1.clone().unwrap_or_default(),
                "md5_10m": payload.md5_10m.clone().unwrap_or_default(),
            }),
            current_step,
        )?;
        let upload_id = json_field(&prepare, "upload_id")?.to_owned();
        let block_size = prepare
            .get("block_size")
            .and_then(Value::as_u64)
            .ok_or_else(|| QqOpenApiError::InvalidResponse("block_size".into()))?;
        let chunks = self
            .clients
            .media
            .read_chunks(&resource_ref, block_size)
            .map_err(|error| QqOpenApiError::Media(error.to_string()))?;
        for chunk in chunks {
            let request = QqHttpRequest {
                method: HttpMethod::Put,
                url: presigned_url_for(&prepare, chunk.index)?,
                headers: BTreeMap::from([("Content-Length".into(), chunk.bytes.len().to_string())]),
                body: None,
                binary_body: Some(chunk.bytes),
            };
            let response = self.clients.http.send(request)?;
            if !(200..300).contains(&response.status) {
                return Err(QqOpenApiError::HttpStatus {
                    status: response.status,
                    body: response.body,
                });
            }
            self.execute_openapi_json(
                HttpMethod::Post,
                payload
                    .scene
                    .upload_part_finish_path(&payload.target_openid),
                json!({
                    "upload_id": upload_id,
                    "part_index": chunk.index,
                    "block_size": block_size,
                    "md5": chunk.md5,
                }),
                current_step,
            )?;
        }
        self.exchange_upload_id(&payload, &upload_id, current_step)
    }

    fn execute_openapi_json(
        &mut self,
        method: HttpMethod,
        path: String,
        body: Value,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let url = absolute_url(&self.config.openapi_base_url, &path);
        let mut refreshed_for_401 = false;
        let max_attempts = self.config.max_retry_attempts.max(1);
        for attempt in 1..=max_attempts {
            let token =
                self.auth
                    .bearer_token(&self.config, self.clients.http.as_mut(), current_step)?;
            let mut request = request_json(method.clone(), url.clone(), body.clone());
            request
                .headers
                .insert("Authorization".into(), authorization_header(&token));
            let response = self.clients.http.send(request);
            match response {
                Ok(response) if (200..300).contains(&response.status) => return Ok(response.body),
                Ok(response) if response.status == 401 && !refreshed_for_401 => {
                    refreshed_for_401 = true;
                    self.auth.invalidate();
                    continue;
                }
                Ok(response) => {
                    let error = QqOpenApiError::HttpStatus {
                        status: response.status,
                        body: response.body,
                    };
                    if error.retryable() && attempt < max_attempts {
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => {
                    if error.retryable() && attempt < max_attempts {
                        continue;
                    }
                    return Err(error);
                }
            }
        }
        Err(QqOpenApiError::InvalidResponse("retry exhausted".into()))
    }
}

fn ensure_file_info(body: &Value) -> Result<(), QqOpenApiError> {
    json_field(body, "file_info").map(|_| ())
}

fn absolute_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.into()
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn ensure_msg_seq(body: &mut Value, msg_seq: u64) {
    let Value::Object(map) = body else {
        return;
    };
    if !map.contains_key("msg_seq") {
        map.insert("msg_seq".into(), json!(msg_seq));
    }
}

fn insert_optional(body: &mut Value, key: &str, value: Option<String>) {
    if let (Value::Object(map), Some(value)) = (body, value) {
        map.insert(key.into(), Value::String(value));
    }
}

fn presigned_url_for(prepare: &Value, index: u64) -> Result<String, QqOpenApiError> {
    prepare
        .get("parts")
        .and_then(Value::as_array)
        .and_then(|parts| {
            parts.iter().find_map(|part| {
                let part_index = part.get("index").and_then(Value::as_u64)?;
                if part_index == index {
                    part.get("presigned_url")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| QqOpenApiError::InvalidResponse("parts.presigned_url".into()))
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
