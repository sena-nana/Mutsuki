//! Host-neutral WebExtension authoring surface.
//!
//! Plugin crates depend on this API. The Host registry, budgets and reactor
//! dispatch live in `mutsuki-web-extension`.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate
)]

mod manifest;
mod rpc;

pub use manifest::{
    BUNDLED_ENTRY_ASSET, ManifestError, content_hash, load_bundled_manifest, load_manifest,
    validate_manifest,
};
pub use rpc::{AsyncRpcHandler, EventRegistry, RpcCallContext, RpcFuture, RpcHandler, RpcRegistry};

use mutsuki_web_protocol::{ExtensionManifest, WebFrontendAssets};

/// Extension identity descriptor used by the host registry.
pub type WebExtensionDescriptor = ExtensionManifest;

/// Backend-facing extension contract. Frontend Vue setup lives in precompiled ESM.
pub trait WebExtension: Send + Sync {
    fn descriptor(&self) -> WebExtensionDescriptor;
    fn frontend_assets(&self) -> Option<WebFrontendAssets>;
    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError>;
    fn register_events(&self, ctx: &mut EventRegistry) -> Result<(), ExtensionError>;
    /// Extra https `img-src` / `media-src` hosts required by this extension's UI.
    fn extra_img_src(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    #[error("rpc failed ({code}): {message}")]
    Rpc { code: String, message: String },
    #[error("extension registration failed: {0}")]
    Registration(String),
    #[error("extension budget exceeded: {0}")]
    Budget(String),
    #[error("extension manifest invalid: {0}")]
    Manifest(String),
    #[error("extension setup failed: {0}")]
    Setup(String),
}

impl ExtensionError {
    /// Stable protocol code exposed to WebBridge callers.
    pub fn rpc_code(&self) -> &str {
        match self {
            Self::CapabilityDenied(_) => "capability_denied",
            Self::Rpc { code, .. } => code,
            _ => "rpc_failed",
        }
    }
}
