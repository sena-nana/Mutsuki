//! Typed protocol contracts for MutsukiWebHost.
//!
//! Frontends talk only through RPC/Event envelopes. Host never exposes Axum/Hyper
//! types as a stable plugin ABI.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::must_use_candidate
)]

mod budgets;
mod error;
mod message;
mod versions;

pub use budgets::{DEFAULT_BUDGETS, ResourceBudgets};
pub use error::{ProtocolError, ProtocolResult};
pub use message::{
    EventEnvelope, EventSubscription, JsonValue, RpcErrorBody, RpcRequest, RpcResponse,
    SessionInfo, WireMessage,
};
pub use versions::{
    EXTENSION_MANIFEST_VERSION, WEB_PROTOCOL_VERSION, WEB_PROTOCOL_VERSION_MAJOR,
    WEB_PROTOCOL_VERSION_MINOR,
};

use serde::{Deserialize, Serialize};

/// Deployment mode shared by embedded and standalone processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    /// WebHost lives inside the Bot/Service process.
    Embedded,
    /// WebHost runs as a separate process and bridges via MutsukiLink/local IPC.
    Standalone,
}

/// Lifecycle status of a WebHost instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebHostStatus {
    Created,
    Starting,
    Running,
    Draining,
    Stopped,
    Failed,
}

/// Structured host health snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebHostStatusReport {
    pub status: WebHostStatus,
    pub healthy: bool,
    pub mode: DeploymentMode,
    pub listen_addr: Option<String>,
    pub application_id: Option<String>,
    pub protocol_version: String,
    pub active_connections: u64,
    pub active_sessions: u64,
    pub extensions_loaded: u64,
    pub extensions_failed: u64,
    pub safe_mode: bool,
    pub error: Option<String>,
}

/// Application identity and branding metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebApplicationDescriptor {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(default)]
    pub theme: Option<String>,
}

/// Precompiled shell assets served at `/` and `/assets/*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebShellAssets {
    pub root_dir: std::path::PathBuf,
    pub index_file: String,
    #[serde(default)]
    pub import_map: serde_json::Map<String, serde_json::Value>,
}

/// Precompiled frontend assets for one extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebFrontendAssets {
    pub manifest: ExtensionManifest,
    pub root_dir: std::path::PathBuf,
}

/// Versioned extension manifest shipped with static assets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub manifest_version: u32,
    pub id: String,
    pub version: String,
    pub entry: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub assets: Vec<AssetEntry>,
    #[serde(default)]
    pub protocol_version: String,
}

/// Hashed static asset entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetEntry {
    pub path: String,
    pub content_hash: String,
    pub bytes: u64,
}

/// Declared contribution point for a frontend extension registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPointKind {
    Page,
    Navigation,
    Slot,
    DashboardCard,
    ConfigRenderer,
    SettingsSection,
    Command,
    EventSubscription,
}

/// Structured failure when an extension load or setup fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionFailure {
    pub extension_id: String,
    pub stage: String,
    pub message: String,
    #[serde(default)]
    pub code: Option<String>,
}
