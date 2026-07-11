use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::api::{
    HttpMethod, QqAuthManager, QqCredentialProvider, QqHttpClient, QqOpenApiError,
    authorization_header, request_empty, request_json,
};
use crate::config::QqBotConfig;

pub struct QqOpenApiTransport {
    config: QqBotConfig,
    auth: QqAuthManager,
    credentials: Arc<dyn QqCredentialProvider>,
    http: Box<dyn QqHttpClient>,
}

impl QqOpenApiTransport {
    pub fn new(
        config: QqBotConfig,
        http: Box<dyn QqHttpClient>,
        credentials: Arc<dyn QqCredentialProvider>,
    ) -> Self {
        Self::new_with_auth(config, http, credentials, QqAuthManager::new())
    }

    pub fn new_with_auth(
        config: QqBotConfig,
        http: Box<dyn QqHttpClient>,
        credentials: Arc<dyn QqCredentialProvider>,
        auth: QqAuthManager,
    ) -> Self {
        Self {
            config,
            auth,
            credentials,
            http,
        }
    }

    pub fn execute_json(
        &mut self,
        method: HttpMethod,
        path: String,
        body: Value,
    ) -> Result<Value, QqOpenApiError> {
        let url = openapi_url(&self.config.openapi_base_url, &path)?;
        let mut refreshed_for_401 = false;
        let max_attempts = self.config.max_retry_attempts.max(1);
        let mut transient_attempt = 1_u8;
        loop {
            let token = self.auth.bearer_token(
                &self.config,
                self.credentials.as_ref(),
                self.http.as_mut(),
            )?;
            let mut request = match &method {
                HttpMethod::Get => request_empty(method.clone(), url.clone()),
                _ => request_json(method.clone(), url.clone(), body.clone()),
            };
            request
                .headers
                .insert("Authorization".into(), authorization_header(&token));
            let response = self.http.send(request);
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
                        headers: response.headers,
                        body: response.body,
                    };
                    if error.retryable() && transient_attempt < max_attempts {
                        self.backoff(transient_attempt, error.retry_after_ms());
                        transient_attempt = transient_attempt.saturating_add(1);
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => {
                    if error.retryable() && transient_attempt < max_attempts {
                        self.backoff(transient_attempt, error.retry_after_ms());
                        transient_attempt = transient_attempt.saturating_add(1);
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    pub fn access_token(&mut self) -> Result<String, QqOpenApiError> {
        self.auth
            .bearer_token(&self.config, self.credentials.as_ref(), self.http.as_mut())
    }

    pub fn invalidate_token(&self) {
        self.auth.invalidate();
    }

    pub fn http(&mut self) -> &mut dyn QqHttpClient {
        self.http.as_mut()
    }

    pub fn config(&self) -> &QqBotConfig {
        &self.config
    }

    fn backoff(&self, attempt: u8, server_delay_ms: Option<u64>) {
        let exponent = u32::from(attempt.saturating_sub(1)).min(20);
        let configured = self
            .config
            .retry_base_delay_ms
            .saturating_mul(1_u64 << exponent)
            .min(self.config.retry_max_delay_ms);
        let delay = server_delay_ms
            .unwrap_or(configured)
            .min(self.config.retry_max_delay_ms);
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
    }
}

fn openapi_url(base: &str, path: &str) -> Result<String, QqOpenApiError> {
    if path.trim().is_empty()
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("//")
        || path.contains(['\r', '\n'])
    {
        return Err(QqOpenApiError::InvalidPayload(
            "OpenAPI path must be a non-empty relative path".into(),
        ));
    }
    let base = Url::parse(&format!("{}/", base.trim_end_matches('/')))
        .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
    base.join(path.trim_start_matches('/'))
        .map(|url| url.to_string())
        .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))
}
