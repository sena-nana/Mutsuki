use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

pub const HOST_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct AppId(String);

impl AppId {
    pub fn new(value: impl Into<String>) -> Result<Self, AppDeliveryError> {
        let value = value.into();
        if value.is_empty()
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_')
        {
            return Err(AppDeliveryError::AppNotInstalled);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AppId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIdentity {
    pub app_id: AppId,
    pub instance_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppDescriptor {
    pub app_id: AppId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub launch_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AppDeliveryOptions {
    /// Stable request id for idempotent replay. Generated when omitted.
    pub request_id: Option<String>,
    pub activate_if_offline: bool,
    pub ready_timeout: Duration,
    pub request_timeout: Duration,
    pub persist_on_failure: bool,
}

impl Default for AppDeliveryOptions {
    fn default() -> Self {
        Self {
            request_id: None,
            activate_if_offline: true,
            ready_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(30),
            persist_on_failure: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPhase {
    DraftSaved,
    Connecting,
    TargetActivating,
    TargetReady,
    Negotiating,
    Transmitting,
    Accepted,
    Completed,
    DeliveryFailed,
}

impl DeliveryPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DraftSaved => "draft_saved",
            Self::Connecting => "connecting",
            Self::TargetActivating => "target_activating",
            Self::TargetReady => "target_ready",
            Self::Negotiating => "negotiating",
            Self::Transmitting => "transmitting",
            Self::Accepted => "accepted",
            Self::Completed => "completed",
            Self::DeliveryFailed => "delivery_failed",
        }
    }

    /// Terminal phases may be evicted once the retention budget is exceeded.
    /// In-flight phases are always retained until they become terminal.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Completed | Self::DeliveryFailed
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationReceipt {
    pub app_id: AppId,
    pub instance_id: String,
    pub already_running: bool,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ActivationError {
    #[error("app not installed: {0}")]
    AppNotInstalled(String),
    #[error("activation failed: {0}")]
    ActivationFailed(String),
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppDeliveryError {
    #[error("app not installed")]
    AppNotInstalled,
    #[error("endpoint unavailable")]
    EndpointUnavailable,
    #[error("activation failed: {message}")]
    ActivationFailed { message: String },
    #[error("ready timeout")]
    ReadyTimeout,
    #[error("authentication failed")]
    AuthenticationFailed,
    #[error("protocol incompatible")]
    ProtocolIncompatible,
    #[error("capability unavailable")]
    CapabilityUnavailable,
    #[error("permission denied")]
    PermissionDenied,
    #[error("delivery failed: {message}")]
    DeliveryFailed { message: String },
    #[error("receipt timeout")]
    ReceiptTimeout,
    #[error("cancelled")]
    Cancelled,
}

impl AppDeliveryError {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::AppNotInstalled => "app_not_installed",
            Self::EndpointUnavailable => "endpoint_unavailable",
            Self::ActivationFailed { .. } => "activation_failed",
            Self::ReadyTimeout => "ready_timeout",
            Self::AuthenticationFailed => "authentication_failed",
            Self::ProtocolIncompatible => "protocol_incompatible",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::PermissionDenied => "permission_denied",
            Self::DeliveryFailed { .. } => "delivery_failed",
            Self::ReceiptTimeout => "receipt_timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

impl From<ActivationError> for AppDeliveryError {
    fn from(error: ActivationError) -> Self {
        match error {
            ActivationError::AppNotInstalled(_) => Self::AppNotInstalled,
            ActivationError::ActivationFailed(message) => Self::ActivationFailed { message },
        }
    }
}
