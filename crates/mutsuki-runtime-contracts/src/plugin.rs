use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{RunnerDescriptor, ScalarValue, SurfaceId, TaskDemand};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Abi,
    Process,
    Wasm,
    Python,
    Native,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginArtifact {
    pub artifact_type: ArtifactType,
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionGrant {
    pub effects: Vec<String>,
    pub resources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecyclePolicy {
    pub reload_policy: String,
    pub unload_timeout_ms: u64,
    pub supports_cancel: bool,
    pub supports_dispose: bool,
    pub supports_snapshot: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginProvides {
    pub runners: Vec<RunnerDescriptor>,
    pub task_demands: Vec<TaskDemand>,
    pub resource_schemas: Vec<String>,
    pub resource_providers: Vec<String>,
    pub effects: Vec<String>,
    pub state_schemas: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub plugin_id: String,
    pub version: String,
    pub api_version: String,
    pub artifact: PluginArtifact,
    pub provides: PluginProvides,
    pub requires: Vec<String>,
    pub permissions: PermissionGrant,
    pub lifecycle: LifecyclePolicy,
    pub metadata: BTreeMap<String, ScalarValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeProfile {
    pub profile_id: String,
    pub enabled_plugins: Vec<String>,
    pub bindings: BTreeMap<String, String>,
    pub allow_dynamic_registration: bool,
    pub allow_hot_reload: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractSurfaceKind {
    Runner,
    TaskKind,
    Schema,
    ResourceSchema,
    ResourceProvider,
    Effect,
    TaskDemand,
    StateSchema,
    Lifecycle,
    Permission,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractSurface {
    pub surface_id: SurfaceId,
    pub kind: ContractSurfaceKind,
    pub owner_plugin_id: String,
    pub fingerprint: String,
    pub deprecated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceOccupancy {
    pub surface_id: SurfaceId,
    pub pending_tasks: u64,
    pub running_invocations: u64,
    pub resource_refs: u64,
    pub state_refs: u64,
    pub active_leases: u64,
    pub open_streams: u64,
    pub subscriptions: u64,
    pub timers: u64,
    pub effect_inflight: u64,
}

impl SurfaceOccupancy {
    pub fn is_zero(&self) -> bool {
        self.pending_tasks == 0
            && self.running_invocations == 0
            && self.resource_refs == 0
            && self.state_refs == 0
            && self.active_leases == 0
            && self.open_streams == 0
            && self.subscriptions == 0
            && self.timers == 0
            && self.effect_inflight == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCompatibility {
    Identical,
    Additive,
    Deprecated,
    Removed,
    Breaking,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeLoadPlan {
    pub lock_version: u64,
    pub core_api_version: String,
    pub profile_id: String,
    pub profile_hash: String,
    pub registry_generation: u64,
    pub plugins: Vec<PluginManifest>,
    pub load_order: Vec<String>,
    pub runner_bindings: BTreeMap<String, String>,
    pub contract_surfaces: Vec<ContractSurface>,
}

pub type RuntimeLock = RuntimeLoadPlan;
