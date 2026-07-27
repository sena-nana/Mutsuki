use mutsuki_tauri_bridge::FrontendError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HostError {
    #[error(transparent)]
    RuntimeFailure(mutsuki_runtime_core::RuntimeFailure),
    #[error("runtime failure: {0}")]
    Runtime(String),
    #[error("resource failure: {0}")]
    Resource(String),
    #[error("approval failure: {0}")]
    Approval(String),
    #[error("configuration failure: {0}")]
    Config(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

pub type HostResult<T> = Result<T, HostError>;

impl From<mutsuki_runtime_core::RuntimeFailure> for HostError {
    fn from(error: mutsuki_runtime_core::RuntimeFailure) -> Self {
        Self::RuntimeFailure(error)
    }
}

impl From<mutsuki_tauri_resource::ResourceBridgeError> for HostError {
    fn from(error: mutsuki_tauri_resource::ResourceBridgeError) -> Self {
        Self::Resource(error.to_string())
    }
}

impl From<HostError> for FrontendError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::RuntimeFailure(failure) => failure.error().clone().into(),
            HostError::Runtime(message) => Self::new("runtime.failed", message),
            HostError::Resource(message) => Self::new("resource.failed", message),
            HostError::Approval(message) => Self::new("approval.failed", message),
            HostError::Config(message) => Self::new("config.failed", message),
            HostError::Unsupported(message) => Self::new("unsupported", message),
        }
    }
}
