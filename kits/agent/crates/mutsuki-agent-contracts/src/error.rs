use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type AgentResult<T> = Result<T, AgentError>;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{code}: {message}")]
pub struct AgentError {
    pub code: String,
    pub message: String,
}

impl AgentError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("agent.invalid_input", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("agent.not_found", message)
    }

    pub fn provider_unavailable(message: impl Into<String>) -> Self {
        Self::new("agent.provider_unavailable", message)
    }
}
