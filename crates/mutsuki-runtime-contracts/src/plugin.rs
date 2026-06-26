use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BindingId, ProtocolId, RunnerDescriptor, ScalarValue, SurfaceId};

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
    pub protocols: Vec<ProtocolDescriptor>,
    pub handler_bindings: Vec<HandlerBinding>,
    pub resource_schemas: Vec<String>,
    pub resource_providers: Vec<String>,
    pub effects: Vec<String>,
    pub streams: Vec<String>,
    pub subscriptions: Vec<String>,
    pub timers: Vec<String>,
    pub state_schemas: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolDescriptor {
    pub protocol_id: String,
    pub version: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub error_schema: serde_json::Value,
    pub codec: String,
    pub compatibility: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandlerBinding {
    pub binding_id: BindingId,
    pub plugin_id: String,
    pub protocol_id: String,
    pub target_protocol_id: ProtocolId,
    pub target_runner_hint: Option<String>,
    pub pool_id: String,
    pub priority: i64,
    pub policy: String,
    pub metadata: BTreeMap<String, ScalarValue>,
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
    TaskProtocol,
    Schema,
    ResourceSchema,
    ResourceProvider,
    Effect,
    Stream,
    Subscription,
    Timer,
    Protocol,
    HandlerBinding,
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
    pub ready_tasks: u64,
    pub running_invocations: u64,
    pub resource_refs: u64,
    pub state_refs: u64,
    pub active_leases: u64,
    pub open_streams: u64,
    pub subscriptions: u64,
    pub timers: u64,
    pub effect_inflight: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceOccupancyHandleKind {
    Stream,
    Subscription,
    Timer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceOccupancyHandle {
    pub handle_id: String,
    pub surface_id: SurfaceId,
    pub owner_plugin_id: String,
    pub plugin_generation: u64,
    pub registry_generation: u64,
    pub kind: SurfaceOccupancyHandleKind,
}

impl SurfaceOccupancy {
    pub fn is_zero(&self) -> bool {
        self.ready_tasks == 0
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
