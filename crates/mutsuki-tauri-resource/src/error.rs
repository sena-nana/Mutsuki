use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Error)]
pub enum ResourceBridgeError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("resource is not utf-8 text: {0}")]
    NotUtf8(String),
    #[error("resource token not found or expired: {0}")]
    InvalidToken(String),
    #[error("io error: {0}")]
    Io(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceEntry {
    pub descriptor: mutsuki_runtime_contracts::ResourceRef,
    pub media_type: Option<String>,
    pub path: std::path::PathBuf,
}
