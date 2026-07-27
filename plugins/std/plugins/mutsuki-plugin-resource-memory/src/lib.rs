use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND,
    ERR_RESOURCE_UNSUPPORTED, ExportPlan, PlanReceipt, ReadPlan, ResourceAccess, ResourceId,
    ResourceLifetime, ResourceProviderCompatibility, ResourceProviderReloadPolicy, ResourceRef,
    ResourceSealState, ResourceSemantic, ResourceTypeDescriptor, RuntimeError, ScalarValue,
    SnapshotDescriptor, StreamPlan, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ResourcePlanGateway, ResourceProviderGateway,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.resource.memory";
pub const PROVIDER_ID: &str = "mutsuki.std.resource.memory";

const BLOB_KIND_ID: &str = "mutsuki.resource.memory.blob";
const SNAPSHOT_KIND_ID: &str = "mutsuki.resource.memory.snapshot";
const CAPABILITY_KIND_ID: &str = "mutsuki.resource.memory.capability";

#[derive(Debug)]
struct MemoryResourceEntry {
    descriptor: ResourceRef,
    bytes: Vec<u8>,
}

#[derive(Debug, Default)]
struct MemoryResourceState {
    next_slot: u64,
    resources: BTreeMap<String, MemoryResourceEntry>,
}

#[derive(Debug, Default)]
pub struct MemoryResourceProvider {
    state: Mutex<MemoryResourceState>,
}

impl MemoryResourceProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn create_resource(
        &self,
        kind_id: &str,
        semantic: ResourceSemantic,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let mut state = self.state.lock().expect("memory provider mutex poisoned");
        state.next_slot += 1;
        let ref_id = format!("memory-resource-{}", state.next_slot);
        let descriptor = resource_ref(
            &ref_id,
            kind_id,
            semantic,
            schema,
            1,
            Some(bytes.len() as u64),
        );
        state.resources.insert(
            ref_id,
            MemoryResourceEntry {
                descriptor: descriptor.clone(),
                bytes,
            },
        );
        Ok(descriptor)
    }

    fn with_entry<T>(
        &self,
        resource: &ResourceRef,
        route: &str,
        read: impl FnOnce(&MemoryResourceEntry) -> RuntimeResult<T>,
    ) -> RuntimeResult<T> {
        ensure_provider(resource, route)?;
        let state = self.state.lock().expect("memory provider mutex poisoned");
        let entry = state.resources.get(&resource.ref_id).ok_or_else(|| {
            runtime_failure(
                ERR_RESOURCE_NOT_FOUND,
                format!("{route}.{}", resource.ref_id),
            )
        })?;
        ensure_descriptor_current(resource, &entry.descriptor, route)?;
        read(entry)
    }
}

impl ResourcePlanGateway for MemoryResourceProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        match plan.operation.as_str() {
            "collect" | "get" => self.with_entry(&plan.resource, "resource.memory.read", |entry| {
                Ok(entry.bytes.clone())
            }),
            operation => Err(unsupported("resource.memory.read", operation)),
        }
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let (source_ref, source_version, bytes) =
            self.with_entry(&plan.resource, "resource.memory.snapshot", |entry| {
                Ok((
                    entry.descriptor.clone(),
                    entry.descriptor.version,
                    entry.bytes.clone(),
                ))
            })?;
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
        Err(unsupported("resource.memory.stream", &plan.operation))
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        if plan.target != "inline_utf8" {
            return Err(unsupported("resource.memory.export", &plan.target));
        }
        let (resource_ref, text) =
            self.with_entry(&plan.resource, "resource.memory.export", |entry| {
                let text = std::str::from_utf8(&entry.bytes).map_err(|error| {
                    let mut runtime_error = RuntimeError::new(
                        ERR_RESOURCE_UNSUPPORTED,
                        "runtime.resource_provider.memory",
                        format!("resource.memory.export.{}", plan.resource.ref_id),
                    );
                    runtime_error
                        .evidence
                        .insert("detail".into(), ScalarValue::String(error.to_string()));
                    RuntimeFailure::new(runtime_error)
                })?;
                Ok((entry.descriptor.clone(), text.to_string()))
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
        ensure_provider(&plan.resource, "resource.memory.write")?;
        let mut state = self.state.lock().expect("memory provider mutex poisoned");
        let entry = state
            .resources
            .get_mut(&plan.resource.ref_id)
            .ok_or_else(|| {
                runtime_failure(
                    ERR_RESOURCE_NOT_FOUND,
                    format!("resource.memory.write.{}", plan.resource.ref_id),
                )
            })?;
        ensure_descriptor_current(&plan.resource, &entry.descriptor, "resource.memory.write")?;
        if plan.resource.semantic != ResourceSemantic::CowVersionedState
            || plan.base_version != entry.descriptor.version
            || plan.patch.base_version != entry.descriptor.version
        {
            return Err(runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                format!("resource.memory.write.{}", plan.resource.ref_id),
            ));
        }

        let new_version = entry.descriptor.version + 1;
        entry.bytes = bytes;
        entry.descriptor.version = new_version;
        entry.descriptor.resource_id.version = new_version;
        entry.descriptor.size_hint = Some(entry.bytes.len() as u64);
        let descriptor = entry.descriptor.clone();
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

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        let capability = self.with_entry(&plan.capability, "resource.memory.command", |entry| {
            if entry.descriptor.semantic != ResourceSemantic::CapabilityResource {
                return Err(unsupported(
                    "resource.memory.command",
                    "non_capability_resource",
                ));
            }
            Ok(entry.descriptor.clone())
        })?;
        match plan.operation.as_str() {
            "query" => Ok(PlanReceipt {
                plan_id: plan.plan_id.clone(),
                status: "commanded".into(),
                resource_ref: Some(capability),
                snapshot: None,
                descriptor_updates: Vec::new(),
                new_version: None,
                output: json!({
                    "provider_id": PROVIDER_ID,
                    "operation": plan.operation.clone(),
                    "args": plan.args.clone(),
                    "idempotency_key": plan.idempotency_key.clone(),
                }),
            }),
            operation => Err(unsupported("resource.memory.command", operation)),
        }
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        if batch.rollback_guarantee {
            return Err(unsupported(
                "resource.memory.command_batch",
                "rollback_guarantee",
            ));
        }
        batch
            .commands
            .iter()
            .map(|command| self.execute_command_plan(command))
            .collect()
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        let mut receipts = Vec::new();
        for command in &saga.steps {
            match self.execute_command_plan(command) {
                Ok(receipt) => receipts.push(receipt),
                Err(cause) => {
                    for compensation in saga.compensations.iter().rev() {
                        let _ = self.execute_command_plan(compensation);
                    }
                    let mut runtime_error = RuntimeError::new(
                        "resource.saga_failed",
                        "runtime.resource_provider.memory",
                        format!("resource.memory.saga.{}", saga.saga_id),
                    );
                    runtime_error.cause = Some(Box::new(cause.error().clone()));
                    return Err(RuntimeFailure::new(runtime_error));
                }
            }
        }
        Ok(receipts)
    }
}

impl ResourceProviderGateway for MemoryResourceProvider {
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
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        let kind_id = if kind_id.is_empty() {
            CAPABILITY_KIND_ID
        } else {
            kind_id
        };
        self.create_resource(
            kind_id,
            ResourceSemantic::CapabilityResource,
            schema,
            Vec::new(),
        )
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
        .resource_provider_gateway(PROVIDER_ID, Arc::new(MemoryResourceProvider::new()))
        .resource_type_descriptor(resource_type(
            BLOB_KIND_ID,
            ResourceSemantic::FrozenValue,
            "mutsuki.resource.memory.blob.v1",
            &["collect", "get", "snapshot", "export"],
        ))
        .resource_type_descriptor(resource_type(
            SNAPSHOT_KIND_ID,
            ResourceSemantic::VersionedSnapshot,
            "mutsuki.resource.memory.snapshot.v1",
            &["collect", "get", "export"],
        ))
        .resource_type_descriptor(resource_type(
            CAPABILITY_KIND_ID,
            ResourceSemantic::CapabilityResource,
            "mutsuki.resource.memory.capability.v1",
            &["query"],
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
    size_hint: Option<u64>,
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
        access: ResourceAccess::ProviderRpc {
            provider_id: PROVIDER_ID.into(),
            method: "memory".into(),
        },
        size_hint,
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn ensure_provider(resource: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if resource.provider_id != PROVIDER_ID {
        return Err(unsupported(route, &resource.provider_id));
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
            format!("{route}.{}", requested.ref_id),
        ));
    }
    Ok(())
}

fn unsupported(route: &str, detail: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        ERR_RESOURCE_UNSUPPORTED,
        "runtime.resource_provider.memory",
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

fn runtime_failure(code: &str, route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        code,
        "runtime.resource_provider.memory",
        route,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::PatchDescriptor;

    #[test]
    fn blob_collect_and_inline_utf8_export_work() {
        let provider = MemoryResourceProvider::new();
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
            resource: blob,
            target: "inline_utf8".into(),
            args: Value::Null,
        };
        assert_eq!(
            provider.execute_export_plan(&export).unwrap().output,
            json!("hello")
        );
    }

    #[test]
    fn cow_commit_updates_version_and_rejects_stale_plans() {
        let provider = MemoryResourceProvider::new();
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
    fn snapshot_returns_usable_snapshot_descriptor() {
        let provider = MemoryResourceProvider::new();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let read = ReadPlan {
            plan_id: "snapshot:1".into(),
            resource: blob,
            operation: "collect".into(),
            args: Value::Null,
        };
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
    fn capability_query_batch_and_saga_paths_are_deterministic() {
        let provider = MemoryResourceProvider::new();
        let capability = provider
            .create_capability_resource("memory_query", "memory.query.v1")
            .unwrap();
        let command = CommandPlan {
            plan_id: "command:1".into(),
            capability: capability.clone(),
            operation: "query".into(),
            args: json!({"key": "value"}),
            idempotency_key: Some("query:1".into()),
        };
        assert_eq!(
            provider.execute_command_plan(&command).unwrap().output["provider_id"],
            PROVIDER_ID
        );
        assert_eq!(
            provider
                .execute_command_batch(&CommandBatch {
                    batch_id: "batch:1".into(),
                    commands: vec![command.clone()],
                    rollback_guarantee: false,
                })
                .unwrap()
                .len(),
            1
        );
        let rollback = provider
            .execute_command_batch(&CommandBatch {
                batch_id: "batch:rollback".into(),
                commands: vec![command.clone()],
                rollback_guarantee: true,
            })
            .unwrap_err();
        assert_eq!(rollback.error().code, ERR_RESOURCE_UNSUPPORTED);

        let mut failing = command.clone();
        failing.operation = "missing".into();
        let saga = provider.execute_saga_plan(&SagaPlan {
            saga_id: "saga:1".into(),
            steps: vec![failing],
            compensations: vec![command],
        });
        let error = saga.unwrap_err();
        assert_eq!(error.error().code, "resource.saga_failed");
        assert!(error.error().cause.is_some());
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
