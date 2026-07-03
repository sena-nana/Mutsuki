use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ExecutorId, RefId, ResourceCellId, ResourceLeaseId, TaskId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceId {
    pub kind_id: String,
    pub slot_id: String,
    pub generation: u64,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSemantic {
    FrozenValue,
    VersionedSnapshot,
    ReadOnlyFact,
    CowVersionedState,
    CapabilityResource,
    StreamResource,
    TransactionResource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceProviderReloadPolicy {
    NoLiveResources,
    CompatibleWithoutLeases,
    DrainActiveLeases,
    RestartRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceProviderCompatibility {
    pub schema_version: String,
    pub required_operations: Vec<String>,
    pub preserves_resource_type_id: bool,
    pub accepts_older_generations: bool,
    pub lease_drain_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTypeDescriptor {
    pub kind_id: String,
    pub semantic: ResourceSemantic,
    pub schema: String,
    pub provider_id: String,
    pub operations: Vec<String>,
    pub reload_policy: ResourceProviderReloadPolicy,
    pub compatibility: ResourceProviderCompatibility,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSealState {
    Writable,
    Sealed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLifetime {
    BorrowedUntilTaskEnd,
    LeaseUntil(u64),
    Persistent,
    ExternalManaged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceAccess {
    Inline,
    MmapFile {
        path: String,
        offset: u64,
        len: u64,
        readonly: bool,
    },
    SharedMemory {
        name: String,
        offset: u64,
        len: u64,
        readonly: bool,
    },
    Blob {
        store_id: String,
        key: String,
    },
    Stream {
        endpoint: String,
    },
    ProviderRpc {
        provider_id: String,
        method: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceRef {
    pub ref_id: RefId,
    pub resource_id: ResourceId,
    pub semantic: ResourceSemantic,
    pub provider_id: String,
    pub resource_kind: String,
    pub schema: String,
    pub version: u64,
    pub generation: u64,
    pub access: ResourceAccess,
    pub size_hint: Option<u64>,
    pub content_hash: Option<String>,
    pub lifetime: ResourceLifetime,
    pub lease: Option<LeaseToken>,
    pub seal_state: ResourceSealState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseToken {
    pub token_id: String,
    pub ref_id: RefId,
    pub owner: String,
    pub mode: String,
    pub expires_at_step: Option<u64>,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusiveWriteLease {
    pub token: LeaseToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCellRef {
    pub cell_id: ResourceCellId,
    pub resource_kind: String,
    pub owner_plugin_id: String,
    pub schema: String,
    pub generation: u64,
    pub health: String,
    pub reload_policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLease {
    pub lease_id: ResourceLeaseId,
    pub cell_id: ResourceCellId,
    pub borrower_task_id: TaskId,
    pub borrower_executor_id: ExecutorId,
    pub mode: String,
    pub expires_at_step: Option<u64>,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueStorage {
    InlineSmall,
    LocalValueStore,
    Blob,
    Stream,
    ProviderRpc,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValueRef {
    pub ref_id: RefId,
    pub provider_id: String,
    pub schema: String,
    pub version: u64,
    pub generation: u64,
    pub size_hint: Option<u64>,
    pub content_hash: Option<String>,
    pub lifetime: ResourceLifetime,
    pub storage: ValueStorage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDescriptor {
    pub snapshot_ref: ResourceRef,
    pub source_ref: ResourceRef,
    pub source_version: u64,
    pub snapshot_version: u64,
    pub is_stale: bool,
    pub is_latest: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatchDescriptor {
    pub patch_id: String,
    pub target_ref: ResourceRef,
    pub base_version: u64,
    pub conflict_policy: String,
    pub operations: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadPlan {
    pub plan_id: String,
    pub resource: ResourceRef,
    pub operation: String,
    pub args: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WritePlan {
    pub plan_id: String,
    pub resource: ResourceRef,
    pub base_version: u64,
    pub conflict_policy: String,
    pub patch: PatchDescriptor,
    pub returning: Option<ReadPlan>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamPlan {
    pub plan_id: String,
    pub resource: ResourceRef,
    pub operation: String,
    pub args: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportPlan {
    pub plan_id: String,
    pub resource: ResourceRef,
    pub target: String,
    pub args: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandPlan {
    pub plan_id: String,
    pub capability: ResourceRef,
    pub operation: String,
    pub args: Value,
    pub idempotency_key: Option<String>,
}

/// Experimental provider/workflow descriptor. CoreRuntime does not interpret
/// or execute transaction semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionPlan {
    pub plan_id: String,
    pub operations: Vec<WritePlan>,
    pub strict: bool,
}

/// Experimental provider/workflow descriptor. CoreRuntime does not interpret
/// or execute batch semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandBatch {
    pub batch_id: String,
    pub commands: Vec<CommandPlan>,
    pub rollback_guarantee: bool,
}

/// Experimental provider/workflow descriptor. CoreRuntime does not interpret
/// or execute saga semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SagaPlan {
    pub saga_id: String,
    pub steps: Vec<CommandPlan>,
    pub compensations: Vec<CommandPlan>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanReceipt {
    pub plan_id: String,
    pub status: String,
    pub resource_ref: Option<ResourceRef>,
    pub snapshot: Option<SnapshotDescriptor>,
    pub descriptor_updates: Vec<ResourceRef>,
    pub new_version: Option<u64>,
    pub output: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceValue {
    Inline {
        schema: String,
        value: Value,
        version: u64,
    },
    ValueRef(ValueRef),
    ResourceRef(ResourceRef),
}
