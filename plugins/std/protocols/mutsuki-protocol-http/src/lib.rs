// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::fmt;

use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const VERSION: &str = "0.3.0";
pub const ABI_CODEC: &str = "runtime-task-v1";
pub const REQUEST: &str = "mutsuki.http.request@2";
pub const EFFECT_REQUEST: &str = "effect.mutsuki.http.request@2";
pub const RESPONSE_BODY_SCHEMA: &str = "mutsuki.http.response.body.v2";
pub const PROTOCOL_IDS: &[&str] = &[REQUEST, EFFECT_REQUEST];
/// Upper bound for a response body returned inline as JSON instead of as a resource.
pub const MAX_INLINE_JSON_BYTES: u64 = 2 * 1024 * 1024;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_allowlist: Option<Vec<String>>,
}

/// How the response body travels back to the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum HttpResponseBody {
    /// A blob resource owned by the configured response provider.
    #[default]
    Resource,
    /// Parsed JSON in the task output, capped at [`MAX_INLINE_JSON_BYTES`]. With `select`
    /// only those JSON pointers are returned, keyed by pointer, so fields the caller does
    /// not need (such as credentials) never enter the task output.
    Json {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        select: Vec<String>,
    },
    /// Only the metadata; the body is not read.
    Discard,
}

impl HttpResponseBody {
    #[must_use]
    pub fn is_resource(&self) -> bool {
        matches!(self, Self::Resource)
    }
}

/// Stores the response `Set-Cookie` values named in `cookies` as one `Cookie` header value
/// in a Host secret. The client config must grant capture of `secret_key` for the URL's
/// domain; the cookie values never enter the task output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCookieCapture {
    pub secret_key: String,
    pub cookies: Vec<String>,
    /// Cookies that must all be present for the capture to be stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    #[serde(default)]
    pub method: HttpMethod,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Header name to Host secret key. The value is resolved when the effect runs, so the
    /// secret never appears in the task payload; it is dropped when a redirect leaves the host.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secret_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "HttpResponseBody::is_resource")]
    pub response_body: HttpResponseBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_cookies: Option<HttpCookieCapture>,
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
            secret_headers: BTreeMap::new(),
            body: None,
            response_body: HttpResponseBody::Resource,
            capture_cookies: None,
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
    /// Response headers never include `set-cookie`.
    pub metadata: HttpResponseMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ResourceRef>,
    /// The inline body for [`HttpResponseBody::Json`]; `None` when it is not valid JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    /// The secret key written by [`HttpRequest::capture_cookies`], when it was stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_secret: Option<String>,
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
    SecretDenied,
    SecretUnavailable,
    SecretStoreFailed,
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
            Self::SecretDenied => "http.secret_denied",
            Self::SecretUnavailable => "http.secret_unavailable",
            Self::SecretStoreFailed => "http.secret_store_failed",
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
                "secret_headers": {"type": "object", "additionalProperties": {"type": "string"}},
                "body": {"type": ["object", "null"]},
                "response_body": {
                    "type": "object",
                    "required": ["mode"],
                    "properties": {
                        "mode": {"enum": ["resource", "json", "discard"]},
                        "select": {"type": "array", "items": {"type": "string"}}
                    }
                },
                "capture_cookies": {
                    "type": ["object", "null"],
                    "required": ["secret_key", "cookies"],
                    "properties": {
                        "secret_key": {"type": "string", "minLength": 1},
                        "cookies": {"type": "array", "items": {"type": "string"}},
                        "required": {"type": "array", "items": {"type": "string"}}
                    }
                },
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
                "body": {"type": ["object", "null"]},
                "json": {},
                "captured_secret": {"type": ["string", "null"]}
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
    fn plain_request_serializes_without_the_optional_extensions() {
        let value = serde_json::to_value(HttpRequest::get("https://example.com")).unwrap();
        assert_eq!(
            value,
            json!({"method": "GET", "url": "https://example.com", "headers": {}, "limits": {}})
        );
    }

    #[test]
    fn response_body_modes_round_trip() {
        let request: HttpRequest = serde_json::from_value(json!({
            "url": "https://example.com",
            "response_body": {"mode": "json", "select": ["/code"]}
        }))
        .unwrap();
        assert_eq!(
            request.response_body,
            HttpResponseBody::Json {
                select: vec!["/code".into()]
            }
        );
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
