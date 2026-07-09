//! Desktop resource bridge: provider / preview / file import-export layers.

mod error;
mod import_export;
mod preview;
mod provider;

pub use error::{ResourceBridgeError, ResourceEntry};
pub use import_export::TauriFileImportExport;
pub use preview::TauriPreviewStore;
pub use provider::{PROVIDER_ID, TauriResourceProvider};

use mutsuki_runtime_contracts::ResourceRef;
use mutsuki_runtime_sdk::ResourceProviderGateway;
use mutsuki_tauri_bridge::PreviewHandle;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Host-facing aggregate over the three resource layers.
#[derive(Clone, Debug)]
pub struct TauriResourceStore {
    provider: TauriResourceProvider,
    preview: TauriPreviewStore,
    files: TauriFileImportExport,
}

impl TauriResourceStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let provider = TauriResourceProvider::new(root);
        let files = TauriFileImportExport::new(provider.clone());
        Self {
            provider,
            preview: TauriPreviewStore::new(),
            files,
        }
    }

    pub fn provider(&self) -> Arc<dyn ResourceProviderGateway> {
        Arc::new(self.provider.clone())
    }

    pub async fn import_file(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        self.files.import_file(path).await
    }

    pub async fn create_blob(
        &self,
        schema: &str,
        bytes: Vec<u8>,
        media_type: Option<String>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        self.provider.create_blob(schema, bytes, media_type).await
    }

    pub async fn read_bytes(&self, ref_id: &str) -> Result<Vec<u8>, ResourceBridgeError> {
        self.provider.read_bytes(ref_id).await
    }

    pub async fn read_text(&self, ref_id: &str) -> Result<String, ResourceBridgeError> {
        self.provider.read_text(ref_id).await
    }

    /// UI preview-only write path; not a runtime ResourcePlanGateway write.
    pub async fn write_bytes(
        &self,
        ref_id: &str,
        bytes: Vec<u8>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        self.provider.write_bytes(ref_id, bytes).await
    }

    pub async fn export_to_file(
        &self,
        ref_id: &str,
        target: impl AsRef<Path>,
    ) -> Result<(), ResourceBridgeError> {
        self.files.export_to_file(ref_id, target).await
    }

    pub fn descriptor(&self, ref_id: &str) -> Result<ResourceRef, ResourceBridgeError> {
        self.provider.descriptor(ref_id)
    }

    pub fn create_preview_handle(
        &self,
        ref_id: &str,
        ttl: Duration,
    ) -> Result<PreviewHandle, ResourceBridgeError> {
        self.provider.descriptor(ref_id)?;
        self.preview.create_preview_handle(ref_id, ttl)
    }

    pub fn resolve_preview_token(&self, token: &str) -> Result<String, ResourceBridgeError> {
        self.preview.resolve_preview_token(token)
    }
}
