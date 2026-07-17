use crate::error::{ResourceBridgeError, ResourceEntry};
use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND, ExportPlan, PlanReceipt,
    ReadPlan, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState,
    ResourceSemantic, RuntimeError, SnapshotDescriptor, StreamPlan, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};
use parking_lot::RwLock;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

pub const PROVIDER_ID: &str = "mutsuki.tauri.resource";
const STORE_ID: &str = "mutsuki-tauri-resource";

#[derive(Debug)]
struct ProviderInner {
    root: PathBuf,
    entries: BTreeMap<String, ResourceEntry>,
}

/// Runtime `ResourceProviderGateway` / `ResourcePlanGateway` for desktop host resources.
#[derive(Clone, Debug)]
pub struct TauriResourceProvider {
    inner: Arc<RwLock<ProviderInner>>,
}

impl TauriResourceProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ProviderInner {
                root: root.into(),
                entries: BTreeMap::new(),
            })),
        }
    }

    pub fn entry(&self, ref_id: &str) -> Result<ResourceEntry, ResourceBridgeError> {
        self.inner
            .read()
            .entries
            .get(ref_id)
            .cloned()
            .ok_or_else(|| ResourceBridgeError::NotFound(ref_id.to_string()))
    }

    pub fn descriptor(&self, ref_id: &str) -> Result<ResourceRef, ResourceBridgeError> {
        Ok(self.entry(ref_id)?.descriptor)
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
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        let descriptor = resource_ref(
            &ref_id,
            "blob",
            ResourceSemantic::VersionedSnapshot,
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
        tokio::fs::read(path)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))
    }

    pub async fn read_chunk(
        &self,
        ref_id: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, ResourceBridgeError> {
        let entry = self.entry(ref_id)?;
        let total = entry.descriptor.size_hint.unwrap_or(0);
        if offset > total {
            return Err(ResourceBridgeError::InvalidRange {
                offset,
                length: total,
            });
        }
        tokio::task::spawn_blocking(move || {
            let mut file = std::fs::File::open(&entry.path)
                .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
            let mut bytes = vec![0; length.min(total.saturating_sub(offset) as usize)];
            file.read_exact(&mut bytes)
                .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
            Ok(bytes)
        })
        .await
        .map_err(|error| ResourceBridgeError::Io(error.to_string()))?
    }

    pub fn read_preview(
        &self,
        ref_id: &str,
    ) -> Result<(Vec<u8>, Option<String>), ResourceBridgeError> {
        let entry = self.entry(ref_id)?;
        let bytes = std::fs::read(&entry.path)
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        Ok((bytes, entry.media_type))
    }

    pub async fn read_text(&self, ref_id: &str) -> Result<String, ResourceBridgeError> {
        let bytes = self.read_bytes(ref_id).await?;
        String::from_utf8(bytes).map_err(|_| ResourceBridgeError::NotUtf8(ref_id.to_string()))
    }

    /// UI preview-only write path; not a runtime ResourcePlanGateway write.
    pub async fn write_bytes(
        &self,
        ref_id: &str,
        bytes: Vec<u8>,
    ) -> Result<ResourceRef, ResourceBridgeError> {
        let entry = self.entry(ref_id)?;
        tokio::fs::write(&entry.path, &bytes)
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
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| ResourceBridgeError::Io(error.to_string()))?;
        }
        tokio::fs::write(target, bytes)
            .await
            .map_err(|error| ResourceBridgeError::Io(error.to_string()))
    }
}

impl ResourcePlanGateway for TauriResourceProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        ensure_provider(&plan.resource, "resource.tauri.read")?;
        let entry = runtime_entry(self, &plan.resource.ref_id)?;
        ensure_descriptor_current(&plan.resource, &entry.descriptor, "resource.tauri.read")?;
        std::fs::read(&entry.path).map_err(runtime_io_failure)
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let bytes = self.collect_read_plan(plan)?;
        let snapshot = blocking_create_blob(
            self,
            kind_id,
            ResourceSemantic::VersionedSnapshot,
            schema,
            bytes,
        )?;
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
        ensure_provider(&plan.resource, "resource.tauri.export")?;
        let entry = runtime_entry(self, &plan.resource.ref_id)?;
        ensure_descriptor_current(&plan.resource, &entry.descriptor, "resource.tauri.export")?;
        std::fs::copy(&entry.path, &plan.target).map_err(runtime_io_failure)?;
        Ok(receipt(
            &plan.plan_id,
            "exported",
            Some(entry.descriptor),
            Vec::new(),
            None,
        ))
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        ensure_provider(&plan.resource, "resource.tauri.write")?;
        let mut entry = runtime_entry(self, &plan.resource.ref_id)?;
        ensure_descriptor_current(&plan.resource, &entry.descriptor, "resource.tauri.write")?;
        if plan.base_version != entry.descriptor.version
            || plan.patch.base_version != entry.descriptor.version
        {
            return Err(runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "tauri.resource",
                format!("resource.tauri.write.{}", plan.resource.ref_id),
            ));
        }
        std::fs::write(&entry.path, &bytes).map_err(runtime_io_failure)?;
        let new_version = entry.descriptor.version + 1;
        entry.descriptor.version = new_version;
        entry.descriptor.resource_id.version = new_version;
        entry.descriptor.size_hint = Some(bytes.len() as u64);
        entry.descriptor.content_hash = Some(content_hash(&bytes));
        let descriptor = entry.descriptor.clone();
        self.inner
            .write()
            .entries
            .insert(entry.descriptor.ref_id.clone(), entry);
        Ok(receipt(
            &plan.plan_id,
            "written",
            Some(descriptor.clone()),
            vec![descriptor],
            Some(new_version),
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

impl ResourceProviderGateway for TauriResourceProvider {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        blocking_create_blob(
            self,
            "blob",
            ResourceSemantic::VersionedSnapshot,
            schema,
            bytes,
        )
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        blocking_create_blob(
            self,
            kind_id,
            ResourceSemantic::CowVersionedState,
            schema,
            bytes,
        )
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
    provider: &TauriResourceProvider,
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    bytes: Vec<u8>,
) -> RuntimeResult<ResourceRef> {
    let ref_id = format!("resource:tauri:{}", Uuid::new_v4());
    let key = format!("{}.bin", Uuid::new_v4());
    let (root, path) = {
        let inner = provider.inner.read();
        let root = inner.root.clone();
        (root.clone(), root.join(&key))
    };
    std::fs::create_dir_all(&root).map_err(runtime_io_failure)?;
    std::fs::write(&path, &bytes).map_err(runtime_io_failure)?;
    let descriptor = resource_ref(
        &ref_id,
        kind_id,
        semantic,
        schema,
        bytes.len() as u64,
        content_hash(&bytes),
        key,
    );
    provider.inner.write().entries.insert(
        ref_id,
        ResourceEntry {
            descriptor: descriptor.clone(),
            media_type: None,
            path,
        },
    );
    Ok(descriptor)
}

fn resource_ref(
    ref_id: &str,
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    len: u64,
    hash: String,
    key: String,
) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version: 1,
        },
        semantic,
        provider_id: PROVIDER_ID.into(),
        resource_kind: kind_id.into(),
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

pub(crate) fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn runtime_entry(provider: &TauriResourceProvider, ref_id: &str) -> RuntimeResult<ResourceEntry> {
    provider.entry(ref_id).map_err(|_| {
        runtime_failure(
            ERR_RESOURCE_NOT_FOUND,
            "tauri.resource",
            format!("resource.{ref_id}"),
        )
    })
}

fn ensure_provider(resource: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if resource.provider_id != PROVIDER_ID {
        return Err(runtime_failure(
            "resource.unsupported",
            "tauri.resource",
            format!("{route}.{}", resource.provider_id),
        ));
    }
    Ok(())
}

fn ensure_descriptor_current(
    requested: &ResourceRef,
    current: &ResourceRef,
    route: &str,
) -> RuntimeResult<()> {
    if requested.generation != current.generation
        || requested.resource_id.generation != requested.generation
        || requested.version != current.version
        || requested.resource_id.version != requested.version
    {
        return Err(runtime_failure(
            ERR_RESOURCE_GENERATION_MISMATCH,
            "tauri.resource",
            format!("{route}.{}", requested.ref_id),
        ));
    }
    Ok(())
}

fn runtime_io_failure(error: std::io::Error) -> RuntimeFailure {
    runtime_failure("resource.io", "tauri.resource", error.to_string())
}

fn runtime_failure(
    code: impl Into<String>,
    source: impl Into<String>,
    route: impl Into<String>,
) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(code, source, route))
}

fn receipt(
    plan_id: &str,
    status: &str,
    resource_ref: Option<ResourceRef>,
    descriptor_updates: Vec<ResourceRef>,
    new_version: Option<u64>,
) -> PlanReceipt {
    PlanReceipt {
        plan_id: plan_id.into(),
        status: status.into(),
        resource_ref,
        snapshot: None,
        descriptor_updates,
        new_version,
        output: json!({ "status": status }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::PatchDescriptor;
    use std::env::temp_dir;

    fn test_provider() -> TauriResourceProvider {
        let root = temp_dir().join(format!("mutsuki-tauri-provider-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("provider root");
        TauriResourceProvider::new(root)
    }

    fn write_plan(plan_id: &str, resource: ResourceRef) -> WritePlan {
        WritePlan {
            plan_id: plan_id.into(),
            resource: resource.clone(),
            base_version: resource.version,
            conflict_policy: "replace".into(),
            patch: PatchDescriptor {
                patch_id: format!("patch:{plan_id}"),
                target_ref: resource.clone(),
                base_version: resource.version,
                conflict_policy: "replace".into(),
                operations: json!({"replace": true}),
            },
            returning: None,
        }
    }

    #[test]
    fn created_resources_use_tauri_provider_id() {
        let provider = test_provider();
        let blob = provider
            .create_blob_resource("text/plain", b"hello".to_vec())
            .expect("blob created");
        assert_eq!(blob.provider_id, PROVIDER_ID);
    }

    #[test]
    fn commit_write_plan_emits_descriptor_updates_and_rejects_stale() {
        let provider = test_provider();
        let resource = provider
            .create_cow_state_resource("text_buffer", "text/plain", b"old".to_vec())
            .expect("cow created");
        let write = write_plan("write:1", resource.clone());
        let receipt = provider
            .commit_write_plan(&write, b"new".to_vec())
            .expect("write commits");
        assert_eq!(receipt.new_version, Some(2));
        assert_eq!(receipt.descriptor_updates.len(), 1);
        assert_eq!(receipt.descriptor_updates[0].version, 2);
        assert_eq!(
            receipt.descriptor_updates[0].content_hash,
            Some(content_hash(b"new"))
        );

        let stale = provider
            .commit_write_plan(&write, b"stale".to_vec())
            .expect_err("stale write rejected");
        assert_eq!(stale.error().code, ERR_RESOURCE_GENERATION_MISMATCH);
    }
}
