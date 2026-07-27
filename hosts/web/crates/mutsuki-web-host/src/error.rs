use thiserror::Error;

pub type WebHostResult<T> = Result<T, WebHostError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WebHostError {
    #[error("invalid web host config: {0}")]
    InvalidConfig(String),
    #[error("invalid web host state: {0}")]
    InvalidState(String),
    #[error("web host failed to start: {0}")]
    StartFailed(String),
    #[error("web host failed to stop: {0}")]
    StopFailed(String),
    #[error("web host io error: {0}")]
    Io(String),
    #[error("extension error: {0}")]
    Extension(String),
    #[error("bridge error: {0}")]
    Bridge(String),
}
