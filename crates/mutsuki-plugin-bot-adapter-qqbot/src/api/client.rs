use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::api::{QqMediaProvider, QqOpenApiError};
use crate::config::QqBotConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessToken {
    pub token: String,
    pub expires_at_step: u64,
}

#[derive(Clone, Debug, Default)]
pub struct QqAuthManager {
    token: Option<AccessToken>,
}

impl QqAuthManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate(&mut self) {
        self.token = None;
    }

    pub fn bearer_token(
        &mut self,
        config: &QqBotConfig,
        client: &mut dyn QqHttpClient,
        current_step: u64,
    ) -> Result<String, QqOpenApiError> {
        if let Some(token) = &self.token
            && token.expires_at_step > current_step + config.token_refresh_margin_secs
        {
            return Ok(token.token.clone());
        }
        let response = client.send(request_json(
            HttpMethod::Post,
            config.token_url.clone(),
            json!({
                "appId": config.app_id,
                "clientSecret": config.client_secret,
            }),
        ))?;
        if !(200..300).contains(&response.status) {
            return Err(QqOpenApiError::HttpStatus {
                status: response.status,
                body: response.body,
            });
        }
        let token = json_field(&response.body, "access_token")?.to_owned();
        let expires_in = response
            .body
            .get("expires_in")
            .and_then(Value::as_u64)
            .ok_or_else(|| QqOpenApiError::InvalidResponse("expires_in".into()))?;
        self.token = Some(AccessToken {
            token: token.clone(),
            expires_at_step: current_step + expires_in,
        });
        Ok(token)
    }
}

pub fn authorization_header(token: &str) -> String {
    format!("QQBot {token}")
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

pub fn request_empty(method: HttpMethod, url: impl Into<String>) -> QqHttpRequest {
    QqHttpRequest {
        method,
        url: url.into(),
        headers: BTreeMap::new(),
        body: None,
        binary_body: None,
    }
}

pub fn json_field<'a>(body: &'a Value, field: &str) -> Result<&'a str, QqOpenApiError> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| QqOpenApiError::InvalidResponse(field.into()))
}
