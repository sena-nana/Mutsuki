use serde_json::Value;

use crate::api::{
    HttpMethod, QqAuthManager, QqHttpClient, QqOpenApiError, authorization_header, request_json,
};
use crate::config::QqBotConfig;

pub struct QqOpenApiTransport {
    config: QqBotConfig,
    auth: QqAuthManager,
    http: Box<dyn QqHttpClient>,
}

impl QqOpenApiTransport {
    pub fn new(config: QqBotConfig, http: Box<dyn QqHttpClient>) -> Self {
        Self {
            config,
            auth: QqAuthManager::new(),
            http,
        }
    }

    pub fn execute_json(
        &mut self,
        method: HttpMethod,
        path: String,
        body: Value,
        current_step: u64,
    ) -> Result<Value, QqOpenApiError> {
        let url = openapi_url(&self.config.openapi_base_url, &path)?;
        let mut refreshed_for_401 = false;
        let max_attempts = self.config.max_retry_attempts.max(1);
        for attempt in 1..=max_attempts {
            let token = self
                .auth
                .bearer_token(&self.config, self.http.as_mut(), current_step)?;
            let mut request = request_json(method.clone(), url.clone(), body.clone());
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

    pub fn http(&mut self) -> &mut dyn QqHttpClient {
        self.http.as_mut()
    }
}

fn openapi_url(base: &str, path: &str) -> Result<String, QqOpenApiError> {
    if path.starts_with("http://") || path.starts_with("https://") {
        Err(QqOpenApiError::InvalidPayload(
            "OpenAPI path must be relative".into(),
        ))
    } else {
        Ok(format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        ))
    }
}
