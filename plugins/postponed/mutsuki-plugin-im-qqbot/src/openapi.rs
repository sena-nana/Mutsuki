use std::collections::BTreeMap;

use serde_json::{Value, json};
use thiserror::Error;

use crate::auth::{QqAuthManager, authorization_header};
use crate::config::QqBotConfig;
use crate::media::QqMediaProvider;
use crate::payload::{
    InteractionAckPayload, MediaUploadPayload, RecallMessagePayload, SendMessagePayload,
    UserShareLinkPayload,
};
use crate::redaction::redact_json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Post,
    Put,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QqHttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub binary_body: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QqHttpResponse {
    pub status: u16,
    pub body: Value,
}

pub trait QqHttpClient: Send {
    fn send(&mut self, request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError>;
}

pub trait QqIdSource: Send {
    fn next_msg_seq(&mut self) -> u64;
}

pub struct QqBotClients {
    pub http: Box<dyn QqHttpClient>,
    pub media: Box<dyn QqMediaProvider>,
}

impl QqBotClients {
    pub fn new(http: Box<dyn QqHttpClient>, media: Box<dyn QqMediaProvider>) -> Self {
        Self { http, media }
    }
}

#[derive(Debug, Error)]
pub enum QqOpenApiError {
    #[error("network error: {0}")]
    Network(String),
    #[error("http status {status}: {body}")]
    HttpStatus { status: u16, body: Value },
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    #[error("media provider failed: {0}")]
    Media(String),
}

impl QqOpenApiError {
    pub fn redacted_message(&self) -> String {
        match self {
            Self::HttpStatus { status, body } => {
                format!("http status {status}: {}", redact_json(body))
            }
            _ => self.to_string(),
        }
    }

    fn retryable(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::HttpStatus { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
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
        let msg_seq = self.id_source.next_msg_seq();
        ensure_msg_seq(&mut body, msg_seq);
        let path = payload.scene.messages_path(&payload.target_openid);
        self.execute_openapi_json(HttpMethod::Post, path, body, current_step)
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
        let path = payload.scene.files_path(&payload.target_openid);
        let response = self.execute_openapi_json(HttpMethod::Post, path, body, current_step)?;
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
        let path = payload
            .scene
            .recall_path(&payload.target_openid, &payload.message_id);
        self.execute_openapi_json(HttpMethod::Delete, path, Value::Null, current_step)
    }

    pub fn ack_interaction(
        &mut self,
        payload: InteractionAckPayload,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        if payload.interaction_id.trim().is_empty() || payload.code > 5 {
            return Err(QqOpenApiError::InvalidPayload(
                "interaction_id and code 0..=5 are required".into(),
            ));
        }
        self.execute_openapi_json(
            HttpMethod::Put,
            format!("/interactions/{}", payload.interaction_id),
            json!({ "code": payload.code }),
            current_step,
        )
    }

    pub fn create_user_share_link(
        &mut self,
        payload: UserShareLinkPayload,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let mut body = json!({});
        insert_optional(&mut body, "callbackData", payload.callback_data);
        self.execute_openapi_json(
            HttpMethod::Post,
            "/v2/bot/share_url".into(),
            body,
            current_step,
        )
    }

    fn exchange_upload_id(
        &mut self,
        payload: &MediaUploadPayload,
        upload_id: &str,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let path = payload.scene.files_path(&payload.target_openid);
        let response = self.execute_openapi_json(
            HttpMethod::Post,
            path,
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
            let presigned_url = presigned_url_for(&prepare, chunk.index)?;
            let request = QqHttpRequest {
                method: HttpMethod::Put,
                url: presigned_url,
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

pub fn request_json(method: HttpMethod, url: impl Into<String>, body: Value) -> QqHttpRequest {
    QqHttpRequest {
        method,
        url: url.into(),
        headers: BTreeMap::from([("Content-Type".into(), "application/json".into())]),
        body: Some(body),
        binary_body: None,
    }
}

pub fn json_field<'a>(body: &'a Value, field: &str) -> Result<&'a str, QqOpenApiError> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| QqOpenApiError::InvalidResponse(field.into()))
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
