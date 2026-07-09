use crate::error::ResourceBridgeError;
use crate::provider::TauriResourceProvider;
use mime_guess::MimeGuess;
use mutsuki_runtime_contracts::ResourceRef;
use std::path::Path;

/// Desktop file import/export affordance; registers descriptors via the provider.
#[derive(Clone, Debug)]
pub struct TauriFileImportExport {
    provider: TauriResourceProvider,
}

impl TauriFileImportExport {
    pub fn new(provider: TauriResourceProvider) -> Self {
        Self { provider }
    }

    pub async fn import_file(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        let path = path.as_ref();
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        let media_type = MimeGuess::from_path(path)
            .first_raw()
            .map(ToString::to_string);
        let schema = media_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        self.provider.create_blob(&schema, bytes, media_type).await
    }

    pub async fn export_to_file(
        &self,
        ref_id: &str,
        target: impl AsRef<Path>,
    ) -> Result<(), ResourceBridgeError> {
        self.provider.export_to_file(ref_id, target).await
    }
}
