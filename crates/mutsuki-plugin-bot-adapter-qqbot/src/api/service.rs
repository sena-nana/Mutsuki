use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::api::{
    HttpMethod, MediaUploadPayload, QqAuthManager, QqBotClients, QqHttpRequest, QqIdSource,
    QqMediaProvider, QqOpenApiError, QqOpenApiTransport, RawCallPayload, RecallMessagePayload,
    SendMessagePayload, json_field,
};
use crate::config::QqBotConfig;

pub struct QqOpenApiService {
    transport: QqOpenApiTransport,
    media: Box<dyn QqMediaProvider>,
    id_source: Box<dyn QqIdSource>,
}

impl QqOpenApiService {
    pub fn account_id(&self) -> &str {
        &self.transport.config().account_id
    }

    pub fn new(config: QqBotConfig, clients: QqBotClients, id_source: Box<dyn QqIdSource>) -> Self {
        Self::new_with_auth(config, clients, id_source, QqAuthManager::new())
    }

    pub fn new_with_auth(
        config: QqBotConfig,
        clients: QqBotClients,
        id_source: Box<dyn QqIdSource>,
        auth: QqAuthManager,
    ) -> Self {
        let QqBotClients {
            http,
            media,
            credentials,
        } = clients;
        Self {
            transport: QqOpenApiTransport::new_with_auth(config, http, credentials, auth),
            media,
            id_source,
        }
    }

    pub fn send_message(&mut self, payload: SendMessagePayload) -> Result<Value, QqOpenApiError> {
        let mut body = payload
            .validated_body()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        ensure_msg_seq(&mut body, self.id_source.next_msg_seq());
        self.transport.execute_json(
            HttpMethod::Post,
            payload.scene.messages_path(&payload.target_openid),
            body,
        )
    }

    pub fn get_account(&mut self) -> Result<Value, QqOpenApiError> {
        let config = self.transport.config().clone();
        let openapi_user =
            self.transport
                .execute_json(HttpMethod::Get, "/users/@me".into(), Value::Null)?;
        Ok(json!({
            "account": {
                "account_id": config.account_id,
                "platform": "qqbot"
            },
            "app_id": config.app_id,
            "openapi_user": openapi_user
        }))
    }

    pub fn gateway_status(&mut self) -> Result<Value, QqOpenApiError> {
        let config = self.transport.config().clone();
        let gateway =
            self.transport
                .execute_json(HttpMethod::Get, "/gateway/bot".into(), Value::Null)?;
        Ok(json!({
            "account_id": config.account_id,
            "platform": "qqbot",
            "gateway": gateway,
            "intents": config.gateway_intents,
            "shard": config.shard
        }))
    }

    pub fn upload_media(&mut self, payload: MediaUploadPayload) -> Result<Value, QqOpenApiError> {
        payload
            .validate()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        if let Some(upload_id) = &payload.upload_id {
            return self.exchange_upload_id(&payload, upload_id);
        }
        if payload.resource_ref.is_some() {
            return self.upload_resource_chunks(payload);
        }
        let mut body = json!({
            "file_type": payload.file_type,
            "srv_send_msg": payload.srv_send_msg.unwrap_or(false),
        });
        insert_optional(&mut body, "url", payload.url);
        insert_optional(&mut body, "file_data", payload.file_data);
        insert_optional(&mut body, "file_name", payload.file_name);
        let response = self.transport.execute_json(
            HttpMethod::Post,
            payload.scene.files_path(&payload.target_openid),
            body,
        )?;
        ensure_file_info(&response)?;
        Ok(response)
    }

    pub fn recall_message(
        &mut self,
        payload: RecallMessagePayload,
    ) -> Result<Value, QqOpenApiError> {
        if payload.target_openid.trim().is_empty() || payload.message_id.trim().is_empty() {
            return Err(QqOpenApiError::InvalidPayload(
                "target_openid and message_id are required".into(),
            ));
        }
        self.transport.execute_json(
            HttpMethod::Delete,
            payload
                .scene
                .recall_path(&payload.target_openid, &payload.message_id),
            Value::Null,
        )
    }

    pub fn raw_call(&mut self, payload: RawCallPayload) -> Result<Value, QqOpenApiError> {
        let method = match payload.method.as_str() {
            "POST" | "post" => HttpMethod::Post,
            "PUT" | "put" => HttpMethod::Put,
            "DELETE" | "delete" => HttpMethod::Delete,
            _ => return Err(QqOpenApiError::InvalidPayload("unsupported method".into())),
        };
        self.transport
            .execute_json(method, payload.path, payload.body.unwrap_or(Value::Null))
    }

    fn exchange_upload_id(
        &mut self,
        payload: &MediaUploadPayload,
        upload_id: &str,
    ) -> Result<Value, QqOpenApiError> {
        let response = self.transport.execute_json(
            HttpMethod::Post,
            payload.scene.files_path(&payload.target_openid),
            json!({ "upload_id": upload_id }),
        )?;
        ensure_file_info(&response)?;
        Ok(response)
    }

    fn upload_resource_chunks(
        &mut self,
        payload: MediaUploadPayload,
    ) -> Result<Value, QqOpenApiError> {
        let resource_ref = payload
            .resource_ref
            .clone()
            .ok_or_else(|| QqOpenApiError::InvalidPayload("resource_ref is required".into()))?;
        let prepare = self.transport.execute_json(
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
        )?;
        let upload_id = json_field(&prepare, "upload_id")?.to_owned();
        let block_size = prepare
            .get("block_size")
            .and_then(Value::as_u64)
            .ok_or_else(|| QqOpenApiError::InvalidResponse("block_size".into()))?;
        let chunks = self
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
            let response = self.transport.http().send(request)?;
            if !(200..300).contains(&response.status) {
                return Err(QqOpenApiError::HttpStatus {
                    status: response.status,
                    headers: response.headers,
                    body: response.body,
                });
            }
            self.transport.execute_json(
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
            )?;
        }
        self.exchange_upload_id(&payload, &upload_id)
    }
}

fn ensure_file_info(body: &Value) -> Result<(), QqOpenApiError> {
    json_field(body, "file_info").map(|_| ())
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
