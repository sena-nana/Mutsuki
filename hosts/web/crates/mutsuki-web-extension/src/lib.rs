//! WebExtension registration surfaces.
//!
//! Extensions contribute through stable registries and explicit extension points.
//! Unrestricted global Vue registration is intentionally not provided.

mod manifest;
mod registry;

pub use manifest::{ManifestError, content_hash, load_manifest, validate_manifest};
pub use registry::{
    Disposable, EventRegistry, ExtensionLoadReport, ExtensionRecord, ExtensionRegistry, RpcHandler,
    RpcRegistry, WebExtensionContext, WebServiceContext,
};

use mutsuki_web_protocol::{ExtensionManifest, WebFrontendAssets};

/// Extension identity descriptor used by the host registry.
pub type WebExtensionDescriptor = ExtensionManifest;

/// Backend-facing extension contract. Frontend Vue setup lives in precompiled ESM.
pub trait WebExtension: Send + Sync {
    fn descriptor(&self) -> WebExtensionDescriptor;
    fn frontend_assets(&self) -> Option<WebFrontendAssets>;
    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError>;
    fn register_events(&self, ctx: &mut EventRegistry) -> Result<(), ExtensionError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    #[error("extension registration failed: {0}")]
    Registration(String),
    #[error("extension budget exceeded: {0}")]
    Budget(String),
    #[error("extension manifest invalid: {0}")]
    Manifest(String),
    #[error("extension setup failed: {0}")]
    Setup(String),
}
