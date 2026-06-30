use mime_guess::MimeGuess;
use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, PlanReceipt, ReadPlan, ResourceAccess, ResourceId,
    ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic, SagaPlan,
    SnapshotDescriptor, StreamPlan, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};
use mutsuki_tauri_bridge::PreviewHandle;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::fs;
use uuid::Uuid;

const PROVIDER_ID: &str = "mutsuki.tauri.resource";
const STORE_ID: &str = "mutsuki-tauri-resource";

#[derive(Clone, Debug, Error)]
pub enum ResourceBridgeError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("resource is not utf-8 text: {0}")]
    NotUtf8(String),
    #[error("resource token not found or expired: {0}")]
    InvalidToken(String),
    #[error("io error: {0}")]
    Io(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceEntry {
    pub descriptor: ResourceRef,
    pub media_type: Option<String>,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
struct PreviewToken {
    ref_id: String,
    expires_at: SystemTime,
}

#[derive(Debug)]
struct ResourceStoreInner {
    root: PathBuf,
    entries: BTreeMap<String, ResourceEntry>,
    previews: BTreeMap<String, PreviewToken>,
}

#[derive(Clone, Debug)]
pub struct TauriResourceStore {
    inner: Arc<RwLock<ResourceStoreInner>>,
}

impl TauriResourceStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResourceStoreInner {
                root: root.into(),
                entries: BTreeMap::new(),
                previews: BTreeMap::new(),
            })),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.inner.read().root.clone()
    }

    pub async fn import_file(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        let path = path.as_ref();
        let bytes = fs::read(path)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        let media_type = MimeGuess::from_path(path)
            .first_raw()
            .map(ToString::to_string);
        let schema = media_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        self.create_blob(&schema, bytes, media_type).await
    }

    pub async fn create_blob(
        &self,
        schema: &str,
        bytes: Vec<u8>,
        media_type: Option<String>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        let ref_id = format!("resource:tauri:{}", Uuid::new_v4());
        let key = format!("{}.bin", Uuid::new_v4());
        let (root, path) = {
            let inner = self.inner.read();
            let root = inner.root.clone();
            (root.clone(), root.join(&key))
        };
        fs::create_dir_all(&root)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        fs::write(&path, &bytes)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        let descriptor = resource_ref(
            &ref_id,
            schema,
            bytes.len() as u64,
            content_hash(&bytes),
            key,
        );
        self.inner.write().entries.insert(
            ref_id,
            ResourceEntry {
                descriptor: descriptor.clone(),
                media_type,
                path,
            },
        );
        Ok(descriptor)
    }

    pub async fn read_bytes(&self, ref_id: &str) -> Result<Vec<u8>, ResourceBridgeError> {
        let path = self.entry(ref_id)?.path;
        fs::read(path)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))
    }

    pub async fn read_text(&self, ref_id: &str) -> Result<String, ResourceBridgeError> {
        let bytes = self.read_bytes(ref_id).await?;
        String::from_utf8(bytes).map_err(|_| ResourceBridgeError::NotUtf8(ref_id.to_string()))
    }

    pub async fn write_bytes(
        &self,
        ref_id: &str,
        bytes: Vec<u8>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        let entry = self.entry(ref_id)?;
        fs::write(&entry.path, &bytes)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        let mut updated = entry.descriptor;
        updated.version += 1;
        updated.resource_id.version = updated.version;
        updated.size_hint = Some(bytes.len() as u64);
        updated.content_hash = Some(content_hash(&bytes));
        self.inner.write().entries.insert(
            ref_id.to_string(),
            ResourceEntry {
                descriptor: updated.clone(),
                media_type: entry.media_type,
                path: entry.path,
            },
        );
        Ok(updated)
    }

    pub async fn export_to_file(
        &self,
        ref_id: &str,
        target: impl AsRef<Path>,
    ) -> Result<(), ResourceBridgeError> {
        let bytes = self.read_bytes(ref_id).await?;
        let target = target.as_ref();
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        }
        fs::write(target, bytes)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))
    }

    pub fn descriptor(&self, ref_id: &str) -> Result<ResourceRef, ResourceBridgeError> {
        Ok(self.entry(ref_id)?.descriptor)
    }

    pub fn list(&self) -> Vec<ResourceRef> {
        self.inner
            .read()
            .entries
            .values()
            .map(|entry| entry.descriptor.clone())
            .collect()
    }

    pub fn create_preview_handle(
        &self,
        ref_id: &str,
        ttl: Duration,
    ) -> Result<PreviewHandle, ResourceBridgeError> {
        self.descriptor(ref_id)?;
        let token = Uuid::new_v4().to_string();
        let expires_at = SystemTime::now() + ttl;
        let expires_at_unix_secs = expires_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.inner.write().previews.insert(
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
        self.cleanup_expired_previews();
        self.inner
            .read()
            .previews
            .get(token)
            .map(|preview| preview.ref_id.clone())
            .ok_or_else(|| ResourceBridgeError::InvalidToken(token.to_string()))
    }

    pub fn revoke_preview_token(&self, token: &str) {
        self.inner.write().previews.remove(token);
    }

    pub fn cleanup_expired_previews(&self) {
        let now = SystemTime::now();
        self.inner
            .write()
            .previews
            .retain(|_, preview| preview.expires_at > now);
    }

    fn entry(&self, ref_id: &str) -> Result<ResourceEntry, ResourceBridgeError> {
        self.inner
            .read()
            .entries
            .get(ref_id)
            .cloned()
            .ok_or_else(|| ResourceBridgeError::NotFound(ref_id.to_string()))
    }
}

impl ResourcePlanGateway for TauriResourceStore {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        let path = runtime_entry_path(self, &plan.resource.ref_id)?;
        std::fs::read(path).map_err(runtime_io_failure)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let bytes = self.collect_read_plan(plan)?;
        let snapshot = blocking_create_blob(self, kind_id, schema, bytes)?;
        Ok(SnapshotDescriptor {
            snapshot_ref: snapshot,
            source_ref: plan.resource.clone(),
            source_version: plan.resource.version,
            snapshot_version: plan.resource.version,
            is_stale: false,
            is_latest: true,
        })
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Ok(StreamPlan {
            plan_id: format!("stream-plan:{}", plan.plan_id),
            resource: plan.resource.clone(),
            operation: "open_stream".into(),
            args: Value::Null,
        })
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        let path = runtime_entry_path(self, &plan.resource.ref_id)?;
        std::fs::copy(path, &plan.target).map_err(runtime_io_failure)?;
        Ok(receipt(
            &plan.plan_id,
            "exported",
            Some(plan.resource.clone()),
            None,
        ))
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        let mut entry = runtime_entry(self, &plan.resource.ref_id)?;
        std::fs::write(&entry.path, &bytes).map_err(runtime_io_failure)?;
        entry.descriptor.version += 1;
        entry.descriptor.resource_id.version = entry.descriptor.version;
        entry.descriptor.size_hint = Some(bytes.len() as u64);
        entry.descriptor.content_hash = Some(content_hash(&bytes));
        self.inner
            .write()
            .entries
            .insert(entry.descriptor.ref_id.clone(), entry.clone());
        Ok(receipt(
            &plan.plan_id,
            "written",
            Some(entry.descriptor),
            Some(plan.base_version + 1),
        ))
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(runtime_failure(
            "resource.command_unsupported",
            "tauri.resource",
            format!("command.{}", plan.operation),
        ))
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        batch
            .commands
            .iter()
            .map(|command| self.execute_command_plan(command))
            .collect()
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        saga.steps
            .iter()
            .map(|command| self.execute_command_plan(command))
            .collect()
    }
}

impl ResourceProviderGateway for TauriResourceStore {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        blocking_create_blob(self, "blob", schema, bytes)
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        blocking_create_blob(self, kind_id, schema, bytes)
    }

    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        let ref_id = format!("resource:tauri:{}", Uuid::new_v4());
        let descriptor = ResourceRef {
            ref_id: ref_id.clone(),
            resource_id: ResourceId {
                kind_id: kind_id.into(),
                slot_id: ref_id,
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::CapabilityResource,
            provider_id: PROVIDER_ID.into(),
            resource_kind: kind_id.into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: PROVIDER_ID.into(),
                method: "command".into(),
            },
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        };
        Ok(descriptor)
    }
}

fn blocking_create_blob(
    store: &TauriResourceStore,
    kind_id: &str,
    schema: &str,
    bytes: Vec<u8>,
) -> RuntimeResult<ResourceRef> {
    let ref_id = format!("resource:tauri:{}", Uuid::new_v4());
    let key = format!("{}.bin", Uuid::new_v4());
    let (root, path) = {
        let inner = store.inner.read();
        let root = inner.root.clone();
        (root.clone(), root.join(&key))
    };
    std::fs::create_dir_all(&root).map_err(runtime_io_failure)?;
    std::fs::write(&path, &bytes).map_err(runtime_io_failure)?;
    let mut descriptor = resource_ref(
        &ref_id,
        schema,
        bytes.len() as u64,
        content_hash(&bytes),
        key,
    );
    descriptor.resource_id.kind_id = kind_id.into();
    descriptor.resource_kind = kind_id.into();
    store.inner.write().entries.insert(
        ref_id,
        ResourceEntry {
            descriptor: descriptor.clone(),
            media_type: None,
            path,
        },
    );
    Ok(descriptor)
}

fn resource_ref(ref_id: &str, schema: &str, len: u64, hash: String, key: String) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: "blob".into(),
            slot_id: ref_id.into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::VersionedSnapshot,
        provider_id: PROVIDER_ID.into(),
        resource_kind: "blob".into(),
        schema: schema.into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Blob {
            store_id: STORE_ID.into(),
            key,
        },
        size_hint: Some(len),
        content_hash: Some(hash),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn runtime_entry(store: &TauriResourceStore, ref_id: &str) -> RuntimeResult<ResourceEntry> {
    store.entry(ref_id).map_err(|error| {
        runtime_failure(
            "resource.not_found",
            "tauri.resource",
            format!("resource.{error}"),
        )
    })
}

fn runtime_entry_path(store: &TauriResourceStore, ref_id: &str) -> RuntimeResult<PathBuf> {
    Ok(runtime_entry(store, ref_id)?.path)
}

fn runtime_io_failure(error: std::io::Error) -> RuntimeFailure {
    runtime_failure("resource.io", "tauri.resource", error.to_string())
}

fn runtime_failure(
    code: impl Into<String>,
    source: impl Into<String>,
    route: impl Into<String>,
) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        code, source, route,
    ))
}

fn receipt(
    plan_id: &str,
    status: &str,
    resource_ref: Option<ResourceRef>,
    new_version: Option<u64>,
) -> PlanReceipt {
    PlanReceipt {
        plan_id: plan_id.into(),
        status: status.into(),
        resource_ref,
        snapshot: None,
        new_version,
        output: json!({ "status": status }),
    }
}
