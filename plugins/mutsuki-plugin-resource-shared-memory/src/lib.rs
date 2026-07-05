use std::collections::BTreeMap;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND,
    ERR_RESOURCE_UNSUPPORTED, ERR_RUNTIME_HOST_FAILED, ExportPlan, PlanReceipt, ReadPlan,
    ResourceAccess, ResourceId, ResourceLifetime, ResourceProviderCompatibility,
    ResourceProviderReloadPolicy, ResourceRef, ResourceSealState, ResourceSemantic,
    ResourceTypeDescriptor, RuntimeError, SagaPlan, ScalarValue, SnapshotDescriptor, StreamPlan,
    WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ResourcePlanGateway, ResourceProviderGateway,
};
use serde_json::{Value, json};
use shared_memory::{Shmem, ShmemConf};

pub const PLUGIN_ID: &str = "mutsuki.std.resource.shared_memory";
pub const PROVIDER_ID: &str = "mutsuki.std.resource.shared_memory";

const BLOB_KIND_ID: &str = "mutsuki.resource.shared_memory.blob";
const SNAPSHOT_KIND_ID: &str = "mutsuki.resource.shared_memory.snapshot";

static PROVIDER_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct OwnedMapping {
    _mapping: Shmem,
}

// `Shmem` contains a raw mapping pointer, so auto traits cannot prove that moving it is safe.
// The provider only accesses mappings while holding its mutex and exposes bytes through copies.
unsafe impl Send for OwnedMapping {}

struct SharedMemoryResourceEntry {
    descriptor: ResourceRef,
    _mapping: Option<OwnedMapping>,
}

#[derive(Default)]
struct SharedMemoryResourceState {
    next_slot: u64,
    resources: BTreeMap<String, SharedMemoryResourceEntry>,
}

pub struct SharedMemoryResourceProvider {
    instance_id: u64,
    state: Mutex<SharedMemoryResourceState>,
}

impl Default for SharedMemoryResourceProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedMemoryResourceProvider {
    pub fn new() -> Self {
        Self {
            instance_id: PROVIDER_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(SharedMemoryResourceState::default()),
        }
    }

    fn create_resource(
        &self,
        kind_id: &str,
        semantic: ResourceSemantic,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let mut state = self
            .state
            .lock()
            .expect("shared-memory provider mutex poisoned");
        let (descriptor, mapping) = self.create_mapping_resource_locked(
            &mut state, kind_id, semantic, schema, 1, bytes, true,
        )?;
        state.resources.insert(
            descriptor.ref_id.clone(),
            SharedMemoryResourceEntry {
                descriptor: descriptor.clone(),
                _mapping: Some(mapping),
            },
        );
        Ok(descriptor)
    }

    fn create_mapping_resource_locked(
        &self,
        state: &mut SharedMemoryResourceState,
        kind_id: &str,
        semantic: ResourceSemantic,
        schema: &str,
        version: u64,
        bytes: Vec<u8>,
        readonly: bool,
    ) -> RuntimeResult<(ResourceRef, OwnedMapping)> {
        state.next_slot += 1;
        let ref_id = format!("shared-memory-resource-{}", state.next_slot);
        let mapping_name = format!(
            "/mutsuki_resource_shared_memory_{}_{}_{}",
            process::id(),
            self.instance_id,
            state.next_slot
        );
        let mapping = create_mapping(&mapping_name, &bytes)?;
        let descriptor = resource_ref(
            &ref_id,
            kind_id,
            semantic,
            schema,
            version,
            &mapping_name,
            bytes.len() as u64,
            readonly,
        );
        Ok((descriptor, mapping))
    }

    fn descriptor_for(&self, resource: &ResourceRef, route: &str) -> RuntimeResult<ResourceRef> {
        ensure_provider(resource, route)?;
        ensure_descriptor_self_consistent(resource, route)?;
        let state = self
            .state
            .lock()
            .expect("shared-memory provider mutex poisoned");
        if let Some(entry) = state.resources.get(&resource.ref_id) {
            ensure_descriptor_current(resource, &entry.descriptor, route)?;
            Ok(entry.descriptor.clone())
        } else {
            Ok(resource.clone())
        }
    }

    fn read_resource_bytes(&self, resource: &ResourceRef, route: &str) -> RuntimeResult<Vec<u8>> {
        let descriptor = self.descriptor_for(resource, route)?;
        let (name, offset, len) = shared_memory_access(&descriptor, route)?;
        read_mapping(name, offset, len, route)
    }
}

impl ResourcePlanGateway for SharedMemoryResourceProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        match plan.operation.as_str() {
            "collect" | "get" => {
                self.read_resource_bytes(&plan.resource, "resource.shared_memory.read")
            }
            operation => Err(unsupported("resource.shared_memory.read", operation)),
        }
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let source_ref = self.descriptor_for(&plan.resource, "resource.shared_memory.snapshot")?;
        let source_version = source_ref.version;
        let bytes = self.read_resource_bytes(&source_ref, "resource.shared_memory.snapshot")?;
        let kind_id = if kind_id.is_empty() {
            SNAPSHOT_KIND_ID
        } else {
            kind_id
        };
        let snapshot_ref =
            self.create_resource(kind_id, ResourceSemantic::VersionedSnapshot, schema, bytes)?;
        Ok(SnapshotDescriptor {
            snapshot_ref,
            source_ref,
            source_version,
            snapshot_version: 1,
            is_stale: false,
            is_latest: true,
        })
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unsupported(
            "resource.shared_memory.stream",
            &plan.operation,
        ))
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        if plan.target != "inline_utf8" {
            return Err(unsupported("resource.shared_memory.export", &plan.target));
        }
        let resource_ref = self.descriptor_for(&plan.resource, "resource.shared_memory.export")?;
        let bytes = self.read_resource_bytes(&resource_ref, "resource.shared_memory.export")?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            detailed_failure(
                ERR_RESOURCE_UNSUPPORTED,
                "resource.shared_memory.export",
                error.to_string(),
            )
        })?;
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "exported".into(),
            resource_ref: Some(resource_ref),
            snapshot: None,
            descriptor_updates: Vec::new(),
            new_version: None,
            output: json!(text),
        })
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        ensure_provider(&plan.resource, "resource.shared_memory.write")?;
        ensure_descriptor_self_consistent(&plan.resource, "resource.shared_memory.write")?;
        if plan.resource.semantic != ResourceSemantic::CowVersionedState
            || plan.base_version != plan.resource.version
            || plan.patch.base_version != plan.resource.version
        {
            return Err(runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                format!("resource.shared_memory.write.{}", plan.resource.ref_id),
            ));
        }

        let mut state = self
            .state
            .lock()
            .expect("shared-memory provider mutex poisoned");
        if let Some(entry) = state.resources.get(&plan.resource.ref_id) {
            ensure_descriptor_current(
                &plan.resource,
                &entry.descriptor,
                "resource.shared_memory.write",
            )?;
        }

        let new_version = plan.resource.version + 1;
        let (mut descriptor, mapping) = self.create_mapping_resource_locked(
            &mut state,
            &plan.resource.resource_id.kind_id,
            ResourceSemantic::CowVersionedState,
            &plan.resource.schema,
            new_version,
            bytes,
            false,
        )?;
        descriptor.ref_id = plan.resource.ref_id.clone();
        descriptor.resource_id.slot_id = plan.resource.resource_id.slot_id.clone();
        state.resources.insert(
            descriptor.ref_id.clone(),
            SharedMemoryResourceEntry {
                descriptor: descriptor.clone(),
                _mapping: Some(mapping),
            },
        );

        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "committed".into(),
            resource_ref: Some(descriptor.clone()),
            snapshot: None,
            descriptor_updates: vec![descriptor],
            new_version: Some(new_version),
            output: Value::Null,
        })
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(unsupported("resource.shared_memory.command", "command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported(
            "resource.shared_memory.command_batch",
            "command_batch",
        ))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported("resource.shared_memory.saga", "saga"))
    }
}

impl ResourceProviderGateway for SharedMemoryResourceProvider {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        self.create_resource(BLOB_KIND_ID, ResourceSemantic::FrozenValue, schema, bytes)
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.create_resource(kind_id, ResourceSemantic::CowVersionedState, schema, bytes)
    }

    fn create_capability_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Err(unsupported(
            "resource.shared_memory.capability",
            "capability",
        ))
    }
}

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .resource_provider_gateway(PROVIDER_ID, Arc::new(SharedMemoryResourceProvider::new()))
        .resource_type_descriptor(resource_type(
            BLOB_KIND_ID,
            ResourceSemantic::FrozenValue,
            "mutsuki.resource.shared_memory.blob.v1",
            &["collect", "get", "snapshot", "export"],
        ))
        .resource_type_descriptor(resource_type(
            SNAPSHOT_KIND_ID,
            ResourceSemantic::VersionedSnapshot,
            "mutsuki.resource.shared_memory.snapshot.v1",
            &["collect", "get", "export"],
        ))
}

fn resource_type(
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    operations: &[&str],
) -> ResourceTypeDescriptor {
    ResourceTypeDescriptor {
        kind_id: kind_id.into(),
        semantic,
        schema: schema.into(),
        provider_id: PROVIDER_ID.into(),
        operations: operations
            .iter()
            .map(|operation| (*operation).into())
            .collect(),
        reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
        compatibility: ResourceProviderCompatibility {
            schema_version: "1.0.0".into(),
            required_operations: operations
                .iter()
                .map(|operation| (*operation).into())
                .collect(),
            preserves_resource_type_id: true,
            accepts_older_generations: false,
            lease_drain_required: false,
        },
    }
}

fn resource_ref(
    ref_id: &str,
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    version: u64,
    mapping_name: &str,
    len: u64,
    readonly: bool,
) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version,
        },
        semantic,
        provider_id: PROVIDER_ID.into(),
        resource_kind: kind_id.into(),
        schema: schema.into(),
        version,
        generation: 1,
        access: ResourceAccess::SharedMemory {
            name: mapping_name.into(),
            offset: 0,
            len,
            readonly,
        },
        size_hint: Some(len),
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: if readonly {
            ResourceSealState::Sealed
        } else {
            ResourceSealState::Writable
        },
    }
}

fn create_mapping(name: &str, bytes: &[u8]) -> RuntimeResult<OwnedMapping> {
    let map_size = bytes.len().max(1);
    let mut mapping = ShmemConf::new()
        .os_id(name)
        .size(map_size)
        .create()
        .map_err(|error| {
            detailed_failure(
                ERR_RUNTIME_HOST_FAILED,
                "resource.shared_memory.create",
                error.to_string(),
            )
        })?;
    if !bytes.is_empty() {
        // SAFETY: the mapping was just created by this provider and is not shared with callers yet.
        unsafe {
            mapping.as_slice_mut()[..bytes.len()].copy_from_slice(bytes);
        }
    }
    Ok(OwnedMapping { _mapping: mapping })
}

fn open_mapping(name: &str, route: &str) -> RuntimeResult<Shmem> {
    ShmemConf::new()
        .os_id(name)
        .open()
        .map_err(|error| detailed_failure(ERR_RESOURCE_NOT_FOUND, route, error.to_string()))
}

fn read_mapping(name: &str, offset: u64, len: u64, route: &str) -> RuntimeResult<Vec<u8>> {
    let mapping = open_mapping(name, route)?;
    let offset = offset as usize;
    let len = len as usize;
    if offset
        .checked_add(len)
        .is_none_or(|end| end > mapping.len())
    {
        return Err(detailed_failure(
            ERR_RESOURCE_UNSUPPORTED,
            route,
            "shared-memory descriptor range is outside mapping".to_string(),
        ));
    }
    // SAFETY: bytes are copied out immediately; no borrowed slice crosses the provider boundary.
    Ok(unsafe { mapping.as_slice()[offset..offset + len].to_vec() })
}

fn shared_memory_access<'a>(
    resource: &'a ResourceRef,
    route: &str,
) -> RuntimeResult<(&'a str, u64, u64)> {
    match &resource.access {
        ResourceAccess::SharedMemory {
            name, offset, len, ..
        } => Ok((name, *offset, *len)),
        _ => Err(unsupported(route, "non_shared_memory_resource")),
    }
}

fn ensure_provider(resource: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if resource.provider_id != PROVIDER_ID {
        return Err(unsupported(route, &resource.provider_id));
    }
    Ok(())
}

fn ensure_descriptor_self_consistent(resource: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if resource.resource_id.generation != resource.generation
        || resource.resource_id.version != resource.version
    {
        return Err(runtime_failure(
            ERR_RESOURCE_GENERATION_MISMATCH,
            format!("{route}.{}", resource.ref_id),
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
        || requested.version != current.version
        || requested.resource_id.generation != requested.generation
        || requested.resource_id.version != requested.version
    {
        return Err(runtime_failure(
            ERR_RESOURCE_GENERATION_MISMATCH,
            format!("{route}.{}", requested.ref_id),
        ));
    }
    Ok(())
}

fn unsupported(route: &str, detail: &str) -> RuntimeFailure {
    detailed_failure(ERR_RESOURCE_UNSUPPORTED, route, detail.to_string())
}

fn detailed_failure(code: &str, route: &str, detail: String) -> RuntimeFailure {
    let mut error = RuntimeError::new(code, "runtime.resource_provider.shared_memory", route);
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail));
    RuntimeFailure::new(error)
}

fn runtime_failure(code: &str, route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        code,
        "runtime.resource_provider.shared_memory",
        route,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::PatchDescriptor;

    #[test]
    fn blob_descriptor_uses_shared_memory_access() {
        let provider = SharedMemoryResourceProvider::new();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let ResourceAccess::SharedMemory {
            name,
            offset,
            len,
            readonly,
        } = &blob.access
        else {
            panic!("expected shared-memory access");
        };
        assert!(name.contains("mutsuki_resource_shared_memory"));
        assert_eq!(*offset, 0);
        assert_eq!(*len, 5);
        assert!(*readonly);
    }

    #[test]
    fn same_provider_collect_export_and_snapshot_work() {
        let provider = SharedMemoryResourceProvider::new();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let read = ReadPlan {
            plan_id: "read:1".into(),
            resource: blob.clone(),
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&read).unwrap(), b"hello");

        let export = ExportPlan {
            plan_id: "export:1".into(),
            resource: blob.clone(),
            target: "inline_utf8".into(),
            args: Value::Null,
        };
        assert_eq!(
            provider.execute_export_plan(&export).unwrap().output,
            json!("hello")
        );

        let snapshot = provider
            .snapshot_read_plan(&read, "text_snapshot", "text.snapshot.v1")
            .unwrap();
        assert_eq!(
            snapshot.snapshot_ref.semantic,
            ResourceSemantic::VersionedSnapshot
        );
        let snapshot_read = ReadPlan {
            plan_id: "read:snapshot".into(),
            resource: snapshot.snapshot_ref,
            operation: "get".into(),
            args: Value::Null,
        };
        assert_eq!(
            provider.collect_read_plan(&snapshot_read).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn second_provider_instance_can_open_descriptor_by_name() {
        let owner = SharedMemoryResourceProvider::new();
        let blob = owner
            .create_blob_resource("text.v1", b"hello from shared memory".to_vec())
            .unwrap();
        let reader = SharedMemoryResourceProvider::new();
        let read = ReadPlan {
            plan_id: "read:foreign".into(),
            resource: blob,
            operation: "collect".into(),
            args: Value::Null,
        };

        assert_eq!(
            reader.collect_read_plan(&read).unwrap(),
            b"hello from shared memory"
        );
    }

    #[test]
    fn cow_commit_updates_version_and_rejects_stale_plans() {
        let provider = SharedMemoryResourceProvider::new();
        let state = provider
            .create_cow_state_resource("text_buffer", "text.state.v1", b"old".to_vec())
            .unwrap();
        let write = write_plan("write:1", state);
        let receipt = provider.commit_write_plan(&write, b"new".to_vec()).unwrap();
        assert_eq!(receipt.new_version, Some(2));

        let stale = provider
            .commit_write_plan(&write, b"stale".to_vec())
            .unwrap_err();
        assert_eq!(stale.error().code, ERR_RESOURCE_GENERATION_MISMATCH);
    }

    #[test]
    fn non_utf8_export_is_structured_failure() {
        let provider = SharedMemoryResourceProvider::new();
        let blob = provider
            .create_blob_resource("bytes.v1", vec![0xff, 0xfe])
            .unwrap();
        let export = ExportPlan {
            plan_id: "export:bytes".into(),
            resource: blob,
            target: "inline_utf8".into(),
            args: Value::Null,
        };

        let error = provider.execute_export_plan(&export).unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_UNSUPPORTED);
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
}
