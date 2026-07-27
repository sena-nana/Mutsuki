use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BotProtocolError {
    #[error("missing field: {0}")]
    MissingField(&'static str),
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}
