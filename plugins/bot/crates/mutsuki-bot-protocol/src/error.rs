use thiserror::Error;

pub const QQBOT_OPENAPI_RATE_LIMITED_ERROR: &str = "qqbot.openapi.rate_limited";
pub const QQBOT_OPENAPI_AUTHENTICATION_ERROR: &str = "qqbot.openapi.authentication";
pub const QQBOT_OPENAPI_TRANSIENT_ERROR: &str = "qqbot.openapi.transient";
pub const QQBOT_OPENAPI_INVALID_REQUEST_ERROR: &str = "qqbot.openapi.invalid_request";
pub const QQBOT_OPENAPI_PERMANENT_ERROR: &str = "qqbot.openapi.permanent";
pub const QQBOT_OPENAPI_MEDIA_PROVIDER_ERROR: &str = "qqbot.openapi.media_provider";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BotProtocolError {
    #[error("missing field: {0}")]
    MissingField(&'static str),
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}
