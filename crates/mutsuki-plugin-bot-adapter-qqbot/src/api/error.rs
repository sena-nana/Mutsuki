use serde_json::Value;
use thiserror::Error;

use crate::adapter::redact_json;

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

    pub fn retryable(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::HttpStatus { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }
}
