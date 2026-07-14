use std::collections::BTreeMap;
use std::process;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_UNSUPPORTED, ExportPlan,
    PlanReceipt, ReadPlan, ResourceRef, ResourceSemantic, SnapshotDescriptor, StreamPlan,
    WritePlan,
};
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};
use serde_json::{Value, json};

use crate::constants::{
    BLOB_KIND_ID, ROUTE_CAPABILITY, ROUTE_COMMAND, ROUTE_COMMAND_BATCH, ROUTE_EXPORT, ROUTE_READ,
    ROUTE_SAGA, ROUTE_SNAPSHOT, ROUTE_STREAM, ROUTE_WRITE, SNAPSHOT_KIND_ID,
};
use crate::descriptor::{
    ensure_descriptor_current, ensure_descriptor_self_consistent, ensure_provider, resource_ref,
    shared_memory_access,
};
use crate::error::{detailed_failure, runtime_failure, unsupported};
use crate::mapping::{OwnedMapping, create_mapping, read_mapping};

static MAPPING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct SharedMemoryResourceEntry {
    descriptor: ResourceRef,
    _mapping: OwnedMapping,
}

#[derive(Default)]
struct SharedMemoryResourceState {
    next_slot: u64,
    resources: BTreeMap<String, SharedMemoryResourceEntry>,
}

pub struct SharedMemoryResourceProvider {
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
                _mapping: mapping,
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
        let mapping_name = mapping_name();
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

    fn read_descriptor_bytes(
        &self,
        descriptor: &ResourceRef,
        route: &str,
    ) -> RuntimeResult<Vec<u8>> {
        let (name, offset, len) = shared_memory_access(descriptor, route)?;
        read_mapping(name, offset, len, route)
    }
}

fn mapping_name() -> String {
    let sequence = MAPPING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("/mtk_{:x}_{sequence:x}", process::id())
}

impl ResourcePlanGateway for SharedMemoryResourceProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        match plan.operation.as_str() {
            "collect" | "get" => {
                let descriptor = self.descriptor_for(&plan.resource, ROUTE_READ)?;
                self.read_descriptor_bytes(&descriptor, ROUTE_READ)
            }
            operation => Err(unsupported(ROUTE_READ, operation)),
        }
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let source_ref = self.descriptor_for(&plan.resource, ROUTE_SNAPSHOT)?;
        let source_version = source_ref.version;
        let bytes = self.read_descriptor_bytes(&source_ref, ROUTE_SNAPSHOT)?;
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
        Err(unsupported(ROUTE_STREAM, &plan.operation))
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        if plan.target != "inline_utf8" {
            return Err(unsupported(ROUTE_EXPORT, &plan.target));
        }
        let resource_ref = self.descriptor_for(&plan.resource, ROUTE_EXPORT)?;
        let bytes = self.read_descriptor_bytes(&resource_ref, ROUTE_EXPORT)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            detailed_failure(ERR_RESOURCE_UNSUPPORTED, ROUTE_EXPORT, error.to_string())
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
        ensure_provider(&plan.resource, ROUTE_WRITE)?;
        ensure_descriptor_self_consistent(&plan.resource, ROUTE_WRITE)?;
        if plan.resource.semantic != ResourceSemantic::CowVersionedState
            || plan.base_version != plan.resource.version
            || plan.patch.base_version != plan.resource.version
        {
            return Err(runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                format!("{ROUTE_WRITE}.{}", plan.resource.ref_id),
            ));
        }

        let mut state = self
            .state
            .lock()
            .expect("shared-memory provider mutex poisoned");
        if let Some(entry) = state.resources.get(&plan.resource.ref_id) {
            ensure_descriptor_current(&plan.resource, &entry.descriptor, ROUTE_WRITE)?;
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
                _mapping: mapping,
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
        Err(unsupported(ROUTE_COMMAND, "command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported(ROUTE_COMMAND_BATCH, "command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported(ROUTE_SAGA, "saga"))
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
        Err(unsupported(ROUTE_CAPABILITY, "capability"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::{PatchDescriptor, ResourceAccess};

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
        assert!(name.starts_with("/mtk_"));
        assert!(name.len() <= 30, "shared-memory OS id is too long: {name}");
        assert_eq!(*offset, 0);
        assert_eq!(*len, 5);
        assert!(*readonly);
    }

    #[test]
    fn mapping_names_are_short_and_unique_across_provider_instances() {
        let first = SharedMemoryResourceProvider::new()
            .create_blob_resource("bytes.v1", vec![1])
            .unwrap();
        let second = SharedMemoryResourceProvider::new()
            .create_blob_resource("bytes.v1", vec![2])
            .unwrap();

        let ResourceAccess::SharedMemory {
            name: first_name, ..
        } = first.access
        else {
            panic!("expected first shared-memory access");
        };
        let ResourceAccess::SharedMemory {
            name: second_name, ..
        } = second.access
        else {
            panic!("expected second shared-memory access");
        };
        assert_ne!(first_name, second_name);
        assert!(first_name.len() <= 30);
        assert!(second_name.len() <= 30);
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
