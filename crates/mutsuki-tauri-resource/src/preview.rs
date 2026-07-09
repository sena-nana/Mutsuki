use crate::error::ResourceBridgeError;
use mutsuki_tauri_bridge::PreviewHandle;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Clone, Debug)]
struct PreviewToken {
    ref_id: String,
    expires_at: SystemTime,
}

/// UI-only preview token / `mutsuki-resource://` URL store (not a runtime lease).
#[derive(Clone, Debug, Default)]
pub struct TauriPreviewStore {
    inner: Arc<RwLock<BTreeMap<String, PreviewToken>>>,
}

impl TauriPreviewStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_preview_handle(
        &self,
        ref_id: &str,
        ttl: Duration,
    ) -> Result<PreviewHandle, ResourceBridgeError> {
        let token = Uuid::new_v4().to_string();
        let expires_at = SystemTime::now() + ttl;
        let expires_at_unix_secs = expires_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.inner.write().insert(
            token.clone(),
            PreviewToken {
                ref_id: ref_id.to_string(),
                expires_at,
            },
        );
        Ok(PreviewHandle {
            ref_id: ref_id.to_string(),
            token: token.clone(),
            url: format!("mutsuki-resource://{token}"),
            expires_at_unix_secs,
        })
    }

    pub fn resolve_preview_token(&self, token: &str) -> Result<String, ResourceBridgeError> {
        let now = SystemTime::now();
        let mut inner = self.inner.write();
        inner.retain(|_, preview| preview.expires_at > now);
        inner
            .get(token)
            .map(|preview| preview.ref_id.clone())
            .ok_or_else(|| ResourceBridgeError::InvalidToken(token.to_string()))
    }
}
