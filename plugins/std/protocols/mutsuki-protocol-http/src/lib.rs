use std::collections::BTreeMap;
use std::fmt;

use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const VERSION: &str = "0.2.0";
pub const ABI_CODEC: &str = "runtime-task-v1";
pub const REQUEST: &str = "mutsuki.http.request@2";
pub const EFFECT_REQUEST: &str = "effect.mutsuki.http.request@2";
pub const RESPONSE_BODY_SCHEMA: &str = "mutsuki.http.response.body.v2";
pub const PROTOCOL_IDS: &[&str] = &[REQUEST, EFFECT_REQUEST];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequestLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    #[serde(default)]
    pub method: HttpMethod,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ResourceRef>,
    #[serde(default)]
    pub limits: HttpRequestLimits,
}

impl HttpRequest {
    #[must_use]
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: BTreeMap::new(),
            body: None,
            limits: HttpRequestLimits::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponseMetadata {
    pub status: u16,
    pub final_url: String,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body_bytes: u64,
    pub redirects_followed: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub metadata: HttpResponseMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ResourceRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpErrorCode {
    InvalidRequest,
    InvalidUrl,
    HttpsRequired,
    DomainDenied,
    DnsFailed,
    PrivateAddress,
    ConnectFailed,
    InvalidHeader,
    HeaderTimeout,
    IdleTimeout,
    TotalTimeout,
    RedirectDenied,
    TooManyRedirects,
    BodyTooLarge,
    RequestBodyFailed,
    ResponseFailed,
}

impl HttpErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "http.invalid_request",
            Self::InvalidUrl => "http.invalid_url",
            Self::HttpsRequired => "http.https_required",
            Self::DomainDenied => "http.domain_denied",
            Self::DnsFailed => "http.dns_failed",
            Self::PrivateAddress => "http.private_address",
            Self::ConnectFailed => "http.connect_failed",
            Self::InvalidHeader => "http.invalid_header",
            Self::HeaderTimeout => "http.header_timeout",
            Self::IdleTimeout => "http.idle_timeout",
            Self::TotalTimeout => "http.total_timeout",
            Self::RedirectDenied => "http.redirect_denied",
            Self::TooManyRedirects => "http.too_many_redirects",
            Self::BodyTooLarge => "http.body_too_large",
            Self::RequestBodyFailed => "http.request_body_failed",
            Self::ResponseFailed => "http.response_failed",
        }
    }
}

impl fmt::Display for HttpErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub fn input_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS.contains(&protocol_id).then(|| {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["url"],
            "properties": {
                "method": {"enum": ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"]},
                "url": {"type": "string", "minLength": 1},
                "headers": {"type": "object", "additionalProperties": {"type": "string"}},
                "body": {"type": ["object", "null"]},
                "limits": {"type": "object"}
            }
        })
    })
}

pub fn output_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS.contains(&protocol_id).then(|| {
        json!({
            "type": "object",
            "required": ["metadata"],
            "properties": {
                "metadata": {"type": "object"},
                "body": {"type": ["object", "null"]}
            }
        })
    })
}

pub fn error_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS.contains(&protocol_id).then(|| {
        json!({
            "type": "object",
            "required": ["code", "message"],
            "properties": {
                "code": {"type": "string", "pattern": "^http\\."},
                "message": {"type": "string"},
                "evidence": {"type": "object"}
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_request_rejects_unknown_fields() {
        let error = serde_json::from_value::<HttpRequest>(json!({
            "url": "https://example.com",
            "domain_allowlist": ["example.com"]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn error_codes_are_stable_and_namespaced() {
        assert_eq!(
            HttpErrorCode::PrivateAddress.as_str(),
            "http.private_address"
        );
        assert_eq!(HttpErrorCode::BodyTooLarge.as_str(), "http.body_too_large");
        assert_eq!(
            HttpErrorCode::RedirectDenied.as_str(),
            "http.redirect_denied"
        );
    }
}
