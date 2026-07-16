use std::collections::{BTreeMap, VecDeque};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_UNSUPPORTED,
    ERR_RUNTIME_HOST_FAILED, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, ResourceSemantic,
    SnapshotDescriptor, StreamPlan, WritePlan,
};
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};
use serde_json::{Value, json};

use crate::constants::{
    BLOB_KIND_ID, ROUTE_CAPABILITY, ROUTE_COMMAND, ROUTE_COMMAND_BATCH, ROUTE_CREATE, ROUTE_EXPORT,
    ROUTE_READ, ROUTE_SAGA, ROUTE_SNAPSHOT, ROUTE_STREAM, ROUTE_WRITE, SNAPSHOT_KIND_ID,
};
use crate::descriptor::{
    ResourceSpec, ensure_descriptor_current, ensure_descriptor_self_consistent, ensure_provider,
    shared_memory_access,
};
use crate::error::{detailed_failure, runtime_failure, unsupported};
use crate::mapping::{OwnedMapping, SharedMemoryView, create_mapping};

static MAPPING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Default hard limit for operations that materialize owned bytes.
pub const DEFAULT_MAX_COLLECT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedMemoryProviderConfig {
    /// Provider-side hard cap for `collect` and inline export. A smaller per-plan `max_bytes` may
    /// be supplied in `ReadPlan.args`/`ExportPlan.args`.
    pub max_collect_bytes: u64,
    /// Number of replaced COW generations whose owner mapping remains retained after descriptor
    /// commit. Older mappings are unlinked by GC unless another snapshot/view still owns them.
    pub retained_generations: usize,
}

impl Default for SharedMemoryProviderConfig {
    fn default() -> Self {
        Self {
            max_collect_bytes: DEFAULT_MAX_COLLECT_BYTES,
            retained_generations: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SharedMemoryCopyMetrics {
    pub collect_copied_bytes: u64,
    pub mapped_view_copied_bytes: u64,
    pub snapshot_copied_bytes: u64,
}

#[derive(Default)]
struct CopyMetrics {
    collect_copied_bytes: AtomicU64,
}

struct SharedMemoryResourceEntry {
    descriptor: ResourceRef,
    mapping: Arc<OwnedMapping>,
}

#[derive(Default)]
struct SharedMemoryResourceState {
    next_slot: u64,
    resources: BTreeMap<String, SharedMemoryResourceEntry>,
    retired: VecDeque<SharedMemoryResourceEntry>,
}

pub struct SharedMemoryResourceProvider {
    state: Mutex<SharedMemoryResourceState>,
    config: SharedMemoryProviderConfig,
    metrics: CopyMetrics,
}

impl Default for SharedMemoryResourceProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedMemoryResourceProvider {
    pub fn new() -> Self {
        Self::with_config(SharedMemoryProviderConfig::default())
    }

    pub fn with_config(config: SharedMemoryProviderConfig) -> Self {
        Self {
            state: Mutex::new(SharedMemoryResourceState::default()),
            config,
            metrics: CopyMetrics::default(),
        }
    }

    /// Opens a zero-copy readonly view. Same-provider resources reuse the already open mapping;
    /// foreign provider instances open the OS mapping named by `ResourceAccess::SharedMemory`.
    pub fn mapped_view(&self, resource: &ResourceRef) -> RuntimeResult<SharedMemoryView> {
        ensure_provider(resource, ROUTE_READ)?;
        ensure_descriptor_self_consistent(resource, ROUTE_READ)?;

        let local = {
            let state = self.lock_state(ROUTE_READ)?;
            state
                .resources
                .get(&resource.ref_id)
                .map(|entry| (entry.descriptor.clone(), Arc::clone(&entry.mapping)))
        };

        if let Some((descriptor, mapping)) = local {
            ensure_descriptor_current(resource, &descriptor, ROUTE_READ)?;
            let (_, offset, len) = shared_memory_access(&descriptor, ROUTE_READ)?;
            return SharedMemoryView::from_mapping(descriptor, mapping, offset, len, ROUTE_READ);
        }

        SharedMemoryView::open(resource)
    }

    pub fn copy_metrics(&self) -> SharedMemoryCopyMetrics {
        SharedMemoryCopyMetrics {
            collect_copied_bytes: self.metrics.collect_copied_bytes.load(Ordering::Relaxed),
            mapped_view_copied_bytes: 0,
            snapshot_copied_bytes: 0,
        }
    }

    pub fn reset_copy_metrics(&self) {
        self.metrics
            .collect_copied_bytes
            .store(0, Ordering::Relaxed);
    }

    /// Applies the configured retired-generation retention and returns the number released.
    /// Existing `SharedMemoryView` and readonly snapshot owners keep their mapping alive via RAII.
    pub fn collect_garbage(&self) -> RuntimeResult<usize> {
        let mut state = self.lock_state(ROUTE_WRITE)?;
        Ok(Self::collect_garbage_locked(
            &mut state,
            self.config.retained_generations,
        ))
    }

    pub fn retained_mapping_count(&self) -> RuntimeResult<usize> {
        Ok(self.lock_state(ROUTE_READ)?.retired.len())
    }

    fn create_resource(
        &self,
        kind_id: &str,
        semantic: ResourceSemantic,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let mut state = self.lock_state(ROUTE_CREATE)?;
        let sealed = semantic != ResourceSemantic::CowVersionedState;
        let spec = ResourceSpec {
            kind_id,
            semantic,
            schema,
            generation: 1,
            version: 1,
            sealed,
        };
        let (descriptor, mapping) = self.create_mapping_resource_locked(&mut state, spec, bytes)?;
        state.resources.insert(
            descriptor.ref_id.clone(),
            SharedMemoryResourceEntry {
                descriptor: descriptor.clone(),
                mapping,
            },
        );
        Ok(descriptor)
    }

    fn create_mapping_resource_locked(
        &self,
        state: &mut SharedMemoryResourceState,
        spec: ResourceSpec<'_>,
        bytes: Vec<u8>,
    ) -> RuntimeResult<(ResourceRef, Arc<OwnedMapping>)> {
        state.next_slot = state.next_slot.checked_add(1).ok_or_else(|| {
            detailed_failure(
                ERR_RUNTIME_HOST_FAILED,
                ROUTE_CREATE,
                "shared-memory slot sequence exhausted".to_string(),
            )
        })?;
        let ref_id = format!("shared-memory-resource-{}", state.next_slot);
        let mapping_name = mapping_name();
        let mapping = create_mapping(&mapping_name, &bytes)?;
        let descriptor = spec.resource_ref(&ref_id, &ref_id, &mapping_name, bytes.len() as u64);
        Ok((descriptor, mapping))
    }

    fn descriptor_for(&self, resource: &ResourceRef, route: &str) -> RuntimeResult<ResourceRef> {
        ensure_provider(resource, route)?;
        ensure_descriptor_self_consistent(resource, route)?;
        let state = self.lock_state(route)?;
        if let Some(entry) = state.resources.get(&resource.ref_id) {
            ensure_descriptor_current(resource, &entry.descriptor, route)?;
            Ok(entry.descriptor.clone())
        } else {
            Ok(resource.clone())
        }
    }

    fn collect_garbage_locked(state: &mut SharedMemoryResourceState, retain: usize) -> usize {
        let mut released = 0;
        while state.retired.len() > retain {
            state.retired.pop_front();
            released += 1;
        }
        released
    }

    fn requested_byte_limit(&self, args: &Value, route: &str) -> RuntimeResult<u64> {
        let requested = match args {
            Value::Null => None,
            Value::Object(values) => match values.get("max_bytes") {
                None => None,
                Some(Value::Number(value)) => value.as_u64(),
                Some(_) => {
                    return Err(detailed_failure(
                        ERR_RESOURCE_UNSUPPORTED,
                        route,
                        "max_bytes must be an unsigned integer".to_string(),
                    ));
                }
            },
            _ => {
                return Err(detailed_failure(
                    ERR_RESOURCE_UNSUPPORTED,
                    route,
                    "plan args must be null or an object containing max_bytes".to_string(),
                ));
            }
        };
        let requested = requested.unwrap_or(self.config.max_collect_bytes);
        if requested > self.config.max_collect_bytes {
            return Err(detailed_failure(
                ERR_RESOURCE_UNSUPPORTED,
                route,
                format!(
                    "requested max_bytes {requested} exceeds provider limit {}",
                    self.config.max_collect_bytes
                ),
            ));
        }
        Ok(requested)
    }

    fn ensure_owned_bytes_allowed(
        &self,
        len: usize,
        args: &Value,
        route: &str,
    ) -> RuntimeResult<()> {
        let limit = self.requested_byte_limit(args, route)?;
        if len as u64 > limit {
            return Err(detailed_failure(
                ERR_RESOURCE_UNSUPPORTED,
                route,
                format!("resource length {len} exceeds owned-byte limit {limit}"),
            ));
        }
        Ok(())
    }

    fn lock_state(&self, route: &str) -> RuntimeResult<MutexGuard<'_, SharedMemoryResourceState>> {
        self.state.lock().map_err(|_| {
            detailed_failure(
                ERR_RUNTIME_HOST_FAILED,
                route,
                "shared-memory provider state lock was poisoned".to_string(),
            )
        })
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
                let view = self.mapped_view(&descriptor)?;
                self.ensure_owned_bytes_allowed(view.len(), &plan.args, ROUTE_READ)?;
                let bytes = view.bytes().to_vec();
                self.metrics
                    .collect_copied_bytes
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                Ok(bytes)
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
        let view = self.mapped_view(&plan.resource)?;
        let source_ref = view.descriptor().clone();
        let source_version = source_ref.version;
        let (mapping_name, _, len) = shared_memory_access(&source_ref, ROUTE_SNAPSHOT)?;
        let kind_id = if kind_id.is_empty() {
            SNAPSHOT_KIND_ID
        } else {
            kind_id
        };

        let (snapshot_ref, is_latest) = {
            let mut state = self.lock_state(ROUTE_SNAPSHOT)?;
            state.next_slot = state.next_slot.checked_add(1).ok_or_else(|| {
                detailed_failure(
                    ERR_RUNTIME_HOST_FAILED,
                    ROUTE_SNAPSHOT,
                    "shared-memory slot sequence exhausted".to_string(),
                )
            })?;
            let ref_id = format!("shared-memory-resource-{}", state.next_slot);
            let descriptor = ResourceSpec {
                kind_id,
                semantic: ResourceSemantic::VersionedSnapshot,
                schema,
                generation: 1,
                version: 1,
                sealed: true,
            }
            .resource_ref(&ref_id, &ref_id, mapping_name, len);
            let is_latest = state
                .resources
                .get(&source_ref.ref_id)
                .is_none_or(|entry| entry.descriptor == source_ref);
            state.resources.insert(
                descriptor.ref_id.clone(),
                SharedMemoryResourceEntry {
                    descriptor: descriptor.clone(),
                    mapping: view.mapping(),
                },
            );
            (descriptor, is_latest)
        };

        Ok(SnapshotDescriptor {
            snapshot_ref,
            source_ref,
            source_version,
            snapshot_version: 1,
            is_stale: false,
            is_latest,
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
        let view = self.mapped_view(&resource_ref)?;
        self.ensure_owned_bytes_allowed(view.len(), &plan.args, ROUTE_EXPORT)?;
        let text = std::str::from_utf8(view.bytes()).map_err(|error| {
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

        let new_version = plan.resource.version.checked_add(1).ok_or_else(|| {
            detailed_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                ROUTE_WRITE,
                "resource version exhausted".to_string(),
            )
        })?;
        let new_generation = plan.resource.generation.checked_add(1).ok_or_else(|| {
            detailed_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                ROUTE_WRITE,
                "resource generation exhausted".to_string(),
            )
        })?;
        let mapping_name = mapping_name();
        let mapping = create_mapping(&mapping_name, &bytes)?;
        let descriptor = ResourceSpec {
            kind_id: &plan.resource.resource_id.kind_id,
            semantic: ResourceSemantic::CowVersionedState,
            schema: &plan.resource.schema,
            generation: new_generation,
            version: new_version,
            sealed: false,
        }
        .resource_ref(
            &plan.resource.ref_id,
            &plan.resource.resource_id.slot_id,
            &mapping_name,
            bytes.len() as u64,
        );

        {
            let mut state = self.lock_state(ROUTE_WRITE)?;
            if let Some(entry) = state.resources.get(&plan.resource.ref_id) {
                ensure_descriptor_current(&plan.resource, &entry.descriptor, ROUTE_WRITE)?;
            }
            let old = state.resources.insert(
                descriptor.ref_id.clone(),
                SharedMemoryResourceEntry {
                    descriptor: descriptor.clone(),
                    mapping,
                },
            );
            if let Some(old) = old {
                state.retired.push_back(old);
            }
            // The new descriptor is authoritative in provider state before any old owner is
            // released. Snapshots/views retain their own Arc and therefore outlive provider GC.
            Self::collect_garbage_locked(&mut state, self.config.retained_generations);
        }

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
    fn blob_descriptor_uses_readonly_shared_memory_access() {
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
    fn collect_export_and_zero_copy_snapshot_work() {
        let provider = SharedMemoryResourceProvider::new();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let read = read_plan("read:1", blob.clone());
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

        provider.reset_copy_metrics();
        let snapshot = provider
            .snapshot_read_plan(&read, "text_snapshot", "text.snapshot.v1")
            .unwrap();
        assert_eq!(
            snapshot.snapshot_ref.semantic,
            ResourceSemantic::VersionedSnapshot
        );
        assert_eq!(mapping_id(&blob), mapping_id(&snapshot.snapshot_ref));
        assert_eq!(provider.copy_metrics().snapshot_copied_bytes, 0);
        assert_eq!(
            provider
                .collect_read_plan(&read_plan("read:snapshot", snapshot.snapshot_ref))
                .unwrap(),
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

        assert_eq!(
            reader
                .collect_read_plan(&read_plan("read:foreign", blob))
                .unwrap(),
            b"hello from shared memory"
        );
    }

    #[test]
    fn cow_commit_creates_new_generation_and_rejects_stale_plans() {
        let provider = SharedMemoryResourceProvider::new();
        let state = provider
            .create_cow_state_resource("text_buffer", "text.state.v1", b"old".to_vec())
            .unwrap();
        let old_mapping = mapping_id(&state).to_string();
        let write = write_plan("write:1", state.clone());
        let receipt = provider.commit_write_plan(&write, b"new".to_vec()).unwrap();
        let descriptor = receipt.resource_ref.unwrap();
        assert_eq!(receipt.new_version, Some(2));
        assert_eq!(descriptor.generation, 2);
        assert_eq!(descriptor.resource_id.generation, 2);
        assert_ne!(mapping_id(&descriptor), old_mapping);

        let stale = provider
            .commit_write_plan(&write, b"stale".to_vec())
            .unwrap_err();
        assert_eq!(stale.error().code, ERR_RESOURCE_GENERATION_MISMATCH);
    }

    #[test]
    fn snapshot_owner_survives_source_generation_gc() {
        let provider = SharedMemoryResourceProvider::with_config(SharedMemoryProviderConfig {
            retained_generations: 0,
            ..SharedMemoryProviderConfig::default()
        });
        let state = provider
            .create_cow_state_resource("text_buffer", "text.state.v1", b"old".to_vec())
            .unwrap();
        let snapshot = provider
            .snapshot_read_plan(
                &read_plan("snapshot", state.clone()),
                "text_snapshot",
                "text.snapshot.v1",
            )
            .unwrap();
        provider
            .commit_write_plan(&write_plan("write", state), b"new".to_vec())
            .unwrap();
        assert_eq!(provider.retained_mapping_count().unwrap(), 0);
        assert_eq!(
            provider
                .collect_read_plan(&read_plan("read", snapshot.snapshot_ref))
                .unwrap(),
            b"old"
        );
    }

    #[test]
    fn retired_generation_is_unlinked_after_retention_gc() {
        let provider = SharedMemoryResourceProvider::with_config(SharedMemoryProviderConfig {
            retained_generations: 1,
            ..SharedMemoryProviderConfig::default()
        });
        let generation_one = provider
            .create_cow_state_resource("text_buffer", "text.state.v1", b"one".to_vec())
            .unwrap();
        let generation_two = provider
            .commit_write_plan(
                &write_plan("write:two", generation_one.clone()),
                b"two".to_vec(),
            )
            .unwrap()
            .resource_ref
            .unwrap();
        assert_eq!(provider.retained_mapping_count().unwrap(), 1);
        drop(SharedMemoryView::open(&generation_one).unwrap());

        provider
            .commit_write_plan(
                &write_plan("write:three", generation_two.clone()),
                b"three".to_vec(),
            )
            .unwrap();
        assert_eq!(provider.retained_mapping_count().unwrap(), 1);
        let error = SharedMemoryView::open(&generation_one).unwrap_err();
        assert_eq!(
            error.error().code,
            mutsuki_runtime_contracts::ERR_RESOURCE_NOT_FOUND
        );
        assert_eq!(
            SharedMemoryView::open(&generation_two).unwrap().bytes(),
            b"two"
        );
    }

    #[test]
    fn collect_has_provider_and_per_plan_byte_limits() {
        let provider = SharedMemoryResourceProvider::with_config(SharedMemoryProviderConfig {
            max_collect_bytes: 4,
            retained_generations: 0,
        });
        let blob = provider
            .create_blob_resource("bytes.v1", b"hello".to_vec())
            .unwrap();

        let error = provider
            .collect_read_plan(&read_plan("read:limit", blob.clone()))
            .unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_UNSUPPORTED);

        let mut read = read_plan("read:requested-limit", blob);
        read.args = json!({"max_bytes": 3});
        let error = provider.collect_read_plan(&read).unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_UNSUPPORTED);
    }

    #[test]
    fn mapped_view_keeps_mapping_alive_after_provider_drop() {
        let provider = SharedMemoryResourceProvider::new();
        let blob = provider
            .create_blob_resource("text.v1", b"still mapped".to_vec())
            .unwrap();
        let view = provider.mapped_view(&blob).unwrap();
        drop(provider);

        assert_eq!(view.bytes(), b"still mapped");
        assert_eq!(view.descriptor(), &blob);
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

    fn read_plan(plan_id: &str, resource: ResourceRef) -> ReadPlan {
        ReadPlan {
            plan_id: plan_id.into(),
            resource,
            operation: "collect".into(),
            args: Value::Null,
        }
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

    fn mapping_id(resource: &ResourceRef) -> &str {
        let ResourceAccess::SharedMemory { name, .. } = &resource.access else {
            panic!("expected shared-memory access");
        };
        name
    }
}
