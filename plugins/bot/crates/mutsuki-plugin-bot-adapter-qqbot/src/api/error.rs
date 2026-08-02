use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    QQBOT_OPENAPI_AUTHENTICATION_ERROR, QQBOT_OPENAPI_INVALID_REQUEST_ERROR,
    QQBOT_OPENAPI_MEDIA_PROVIDER_ERROR, QQBOT_OPENAPI_PERMANENT_ERROR,
    QQBOT_OPENAPI_RATE_LIMITED_ERROR, QQBOT_OPENAPI_TRANSIENT_ERROR,
};
use serde_json::Value;
use thiserror::Error;

use crate::adapter::redact_json;

#[derive(Error)]
pub enum QqOpenApiError {
    #[error("network error: {0}")]
    Network(String),
    #[error("http status {status}")]
    HttpStatus {
        status: u16,
        headers: BTreeMap<String, String>,
        body: Value,
    },
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    #[error("media provider failed: {0}")]
    Media(String),
    #[error("QQBot client secret is unavailable from the Host secret boundary")]
    CredentialsUnavailable,
    #[error("HTTP response exceeds configured {limit} byte limit")]
    ResponseTooLarge { limit: usize },
    #[error("system clock error: {0}")]
    Clock(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QqOpenApiErrorClass {
    RateLimited,
    Authentication,
    Transient,
    InvalidRequest,
    Permanent,
    MediaProvider,
}

impl QqOpenApiErrorClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::Authentication => "authentication",
            Self::Transient => "transient",
            Self::InvalidRequest => "invalid_request",
            Self::Permanent => "permanent",
            Self::MediaProvider => "media_provider",
        }
    }
}

impl QqOpenApiError {
    pub fn classification(&self) -> QqOpenApiErrorClass {
        match self {
            Self::Network(_) | Self::Clock(_) => QqOpenApiErrorClass::Transient,
            Self::HttpStatus { status: 429, .. } => QqOpenApiErrorClass::RateLimited,
            Self::HttpStatus {
                status: 401 | 403, ..
            }
            | Self::CredentialsUnavailable => QqOpenApiErrorClass::Authentication,
            Self::HttpStatus { status, .. } if *status >= 500 => QqOpenApiErrorClass::Transient,
            Self::InvalidPayload(_) => QqOpenApiErrorClass::InvalidRequest,
            Self::Media(_) => QqOpenApiErrorClass::MediaProvider,
            Self::HttpStatus { .. } | Self::InvalidResponse(_) | Self::ResponseTooLarge { .. } => {
                QqOpenApiErrorClass::Permanent
            }
        }
    }

    pub fn stable_code(&self) -> &'static str {
        match self.classification() {
            QqOpenApiErrorClass::RateLimited => QQBOT_OPENAPI_RATE_LIMITED_ERROR,
            QqOpenApiErrorClass::Authentication => QQBOT_OPENAPI_AUTHENTICATION_ERROR,
            QqOpenApiErrorClass::Transient => QQBOT_OPENAPI_TRANSIENT_ERROR,
            QqOpenApiErrorClass::InvalidRequest => QQBOT_OPENAPI_INVALID_REQUEST_ERROR,
            QqOpenApiErrorClass::Permanent => QQBOT_OPENAPI_PERMANENT_ERROR,
            QqOpenApiErrorClass::MediaProvider => QQBOT_OPENAPI_MEDIA_PROVIDER_ERROR,
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::HttpStatus { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn redacted_message(&self) -> String {
        match self {
            Self::HttpStatus { status, body, .. } => {
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

    pub fn retry_after_ms(&self) -> Option<u64> {
        let Self::HttpStatus { headers, .. } = self else {
            return None;
        };
        header(headers, "retry-after")
            .and_then(|value| value.trim().parse::<f64>().ok())
            .map(|seconds| (seconds.max(0.0) * 1_000.0) as u64)
            .or_else(|| {
                header(headers, "x-ratelimit-reset-after")
                    .and_then(|value| value.trim().parse::<f64>().ok())
                    .map(|seconds| (seconds.max(0.0) * 1_000.0) as u64)
            })
    }
}

impl std::fmt::Debug for QqOpenApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("QqOpenApiError")
            .field(&self.redacted_message())
            .finish()
    }
}

fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}
