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
    BUNDLED_ENTRY_ASSET, BundledAssets, BundledManifest, ManifestError, content_hash,
    load_bundled_manifest, load_manifest, materialize_bundled_assets, validate_manifest,
};
pub use rpc::{AsyncRpcHandler, EventRegistry, RpcCallContext, RpcFuture, RpcHandler, RpcRegistry};

use mutsuki_web_protocol::{ExtensionManifest, WebFrontendAssets};

/// Extension identity descriptor used by the host registry.
pub type WebExtensionDescriptor = ExtensionManifest;

/// Backend-facing extension contract. Frontend Vue setup lives in precompiled ESM.
pub trait WebExtension: Send + Sync {
    fn descriptor(&self) -> WebExtensionDescriptor;
    /// RPC namespace and event-topic prefix. Defaults to the extension id; owners with a
    /// qualified id (`mutsuki.web.diagnostics`) keep a short namespace (`control`) here.
    fn namespace(&self) -> String {
        self.descriptor().id
    }
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

    /// Message sent to WebBridge callers next to [`Self::rpc_code`]. A coded owner error
    /// carries its own message; the code travels separately, so it is not repeated here.
    pub fn rpc_message(&self) -> String {
        match self {
            Self::Rpc { message, .. } => message.clone(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExtensionError;

    #[test]
    fn coded_errors_keep_code_and_message_apart() {
        let error = ExtensionError::Rpc {
            code: "config.revision_conflict".into(),
            message: "revision conflict: expected 3, current 5".into(),
        };
        assert_eq!(error.rpc_code(), "config.revision_conflict");
        assert_eq!(
            error.rpc_message(),
            "revision conflict: expected 3, current 5"
        );
        let denied = ExtensionError::CapabilityDenied("bot.read".into());
        assert_eq!(denied.rpc_message(), "capability denied: bot.read");
    }
}
