use super::types::AppId;
use mutsuki_runtime_contracts::CapabilityRequestEnvelope;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeliveryDraft {
    pub request_id: String,
    pub source_app: String,
    pub target_app: String,
    pub capability_name: String,
    pub capability_protocol_version: u32,
    pub capability_schema_version: u32,
    pub payload: Value,
    pub saved_at_unix_ms: u64,
    pub reason: String,
    /// Draft is recoverable but must never be presented as delivered.
    pub delivered: bool,
}

impl DeliveryDraft {
    pub fn from_envelope(envelope: &CapabilityRequestEnvelope, reason: impl Into<String>) -> Self {
        Self {
            request_id: envelope.request_id.clone(),
            source_app: envelope.source.clone(),
            target_app: envelope.target.clone(),
            capability_name: envelope.capability.name.clone(),
            capability_protocol_version: envelope.capability.protocol_version,
            capability_schema_version: envelope.capability.schema_version,
            payload: envelope.payload.clone(),
            saved_at_unix_ms: now_unix_ms(),
            reason: reason.into(),
            delivered: false,
        }
    }
}

#[derive(Clone, Default)]
pub struct DeliveryDraftStore {
    inner: Arc<RwLock<BTreeMap<String, DeliveryDraft>>>,
    directory: Option<PathBuf>,
}

impl DeliveryDraftStore {
    pub fn memory() -> Self {
        Self::default()
    }

    pub fn persistent(directory: impl Into<PathBuf>) -> std::io::Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        let store = Self {
            inner: Arc::new(RwLock::new(BTreeMap::new())),
            directory: Some(directory.clone()),
        };
        if directory.is_dir() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let payload = fs::read_to_string(&path)?;
                if let Ok(draft) = serde_json::from_str::<DeliveryDraft>(&payload) {
                    store.inner.write().insert(draft.request_id.clone(), draft);
                }
            }
        }
        Ok(store)
    }

    pub fn save(&self, draft: DeliveryDraft) -> std::io::Result<()> {
        debug_assert!(!draft.delivered, "drafts must never be marked delivered");
        if let Some(directory) = &self.directory {
            write_draft_file(directory, &draft)?;
        }
        self.inner.write().insert(draft.request_id.clone(), draft);
        Ok(())
    }

    pub fn get(&self, request_id: &str) -> Option<DeliveryDraft> {
        self.inner.read().get(request_id).cloned()
    }

    pub fn list_for_target(&self, target: &AppId) -> Vec<DeliveryDraft> {
        self.inner
            .read()
            .values()
            .filter(|draft| draft.target_app == target.as_str() && !draft.delivered)
            .cloned()
            .collect()
    }
}

fn write_draft_file(directory: &Path, draft: &DeliveryDraft) -> std::io::Result<()> {
    let path = directory.join(format!("{}.json", draft.request_id));
    let pending = path.with_extension("json.pending");
    let payload = serde_json::to_vec_pretty(draft)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(&pending, payload)?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(pending, path)?;
    Ok(())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
