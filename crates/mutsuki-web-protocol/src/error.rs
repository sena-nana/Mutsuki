use thiserror::Error;

pub type ProtocolResult<T> = Result<T, ProtocolError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("protocol version mismatch: client={client}, host={host}")]
    VersionMismatch { client: String, host: String },

    #[error("extension manifest version unsupported: {0}")]
    UnsupportedManifestVersion(u32),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),

    #[error("payload exceeds budget: limit={limit} actual={actual}")]
    PayloadTooLarge { limit: usize, actual: usize },

    #[error("queue full: {0}")]
    QueueFull(String),

    #[error("session not authenticated")]
    Unauthenticated,

    #[error("invalid message: {0}")]
    InvalidMessage(String),

    #[error("extension not found: {0}")]
    ExtensionNotFound(String),

    #[error("rpc method not found: {namespace}.{method}")]
    MethodNotFound { namespace: String, method: String },

    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),

    #[error("internal protocol error: {0}")]
    Internal(String),
}
