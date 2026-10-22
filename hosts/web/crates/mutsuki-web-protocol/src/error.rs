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

    /// Hello (or a cookie login) presented credentials the host rejected.
    #[error("authentication failed")]
    AuthFailed,

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

impl ProtocolError {
    /// Stable wire code for `WireMessage::Error`. Clients branch on this, never on the
    /// human-readable message.
    pub fn code(&self) -> &'static str {
        match self {
            Self::VersionMismatch { .. } => "version_mismatch",
            Self::UnsupportedManifestVersion(_) => "unsupported_manifest_version",
            Self::CapabilityDenied(_) => "capability_denied",
            Self::PayloadTooLarge { .. } => "payload_too_large",
            Self::QueueFull(_) => "queue_full",
            Self::Unauthenticated => "unauthenticated",
            Self::AuthFailed => "auth_failed",
            Self::InvalidMessage(_) => "invalid_message",
            Self::ExtensionNotFound(_) => "extension_not_found",
            Self::MethodNotFound { .. } => "method_not_found",
            Self::BudgetExceeded(_) => "budget_exceeded",
            Self::Internal(_) => "internal",
        }
    }
}
