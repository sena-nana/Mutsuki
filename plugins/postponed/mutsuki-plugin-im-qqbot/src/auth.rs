use serde_json::json;

use crate::config::QqBotConfig;
use crate::openapi::{HttpMethod, QqHttpClient, QqOpenApiError, json_field, request_json};

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
        let request = request_json(
            HttpMethod::Post,
            config.token_url.clone(),
            json!({
                "appId": config.app_id,
                "clientSecret": config.client_secret,
            }),
        );
        let response = client.send(request)?;
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
            .and_then(|value| value.as_u64())
            .ok_or_else(|| QqOpenApiError::InvalidResponse("expires_in".into()))?;
        let expires_at_step = current_step + expires_in;
        self.token = Some(AccessToken {
            token: token.clone(),
            expires_at_step,
        });
        Ok(token)
    }
}

pub fn authorization_header(token: &str) -> String {
    format!("QQBot {token}")
}
