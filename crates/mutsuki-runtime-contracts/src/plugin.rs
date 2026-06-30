use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::extension::{
    BridgeDescriptor, CodecDescriptor, HostExtensionDescriptor, PluginBackendDescriptor,
    SchedulerPolicyDescriptor, WorkflowDescriptor,
};
use crate::{
    BindingId, ProtocolId, ResourceTypeDescriptor, RunnerDescriptor, ScalarValue, SurfaceId,
};

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
#[serde(rename_all = "snake_case")]
pub enum PluginDeploymentKind {
    Builtin,
    Abi,
    Wasm,
    Process,
    Python,
}

impl PluginDeploymentKind {
    pub fn default_for_artifact(artifact_type: &ArtifactType) -> Self {
        match artifact_type {
            ArtifactType::Native => Self::Builtin,
            ArtifactType::Abi => Self::Abi,
            ArtifactType::Wasm => Self::Wasm,
            ArtifactType::Process => Self::Process,
            ArtifactType::Python => Self::Python,
        }
    }

    pub fn is_compatible_with_artifact(&self, artifact_type: &ArtifactType) -> bool {
        matches!(
            (self, artifact_type),
            (Self::Builtin, ArtifactType::Native)
                | (Self::Abi, ArtifactType::Abi)
                | (Self::Wasm, ArtifactType::Wasm)
                | (Self::Process, ArtifactType::Process)
                | (Self::Python, ArtifactType::Python)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfileMode {
    FullDev,
    ExtensibleRuntime,
    BuiltinOnly,
    LockedBuiltin,
}

impl Default for RuntimeProfileMode {
    fn default() -> Self {
        Self::FullDev
    }
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
    pub resource_types: Vec<ResourceTypeDescriptor>,
    pub effects: Vec<String>,
    pub streams: Vec<String>,
    pub subscriptions: Vec<String>,
    pub timers: Vec<String>,
    pub state_schemas: Vec<String>,
    pub host_extensions: Vec<HostExtensionDescriptor>,
    pub plugin_backends: Vec<PluginBackendDescriptor>,
    pub codecs: Vec<CodecDescriptor>,
    pub bridges: Vec<BridgeDescriptor>,
    pub scheduler_policies: Vec<SchedulerPolicyDescriptor>,
    pub workflows: Vec<WorkflowDescriptor>,
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
    pub mode: RuntimeProfileMode,
    pub enabled_plugins: Vec<String>,
    pub bindings: BTreeMap<String, String>,
    pub plugin_deployments: BTreeMap<String, PluginDeploymentKind>,
    pub allow_dynamic_registration: bool,
    pub allow_hot_reload: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCapabilityGraph {
    pub profile_mode: RuntimeProfileMode,
    pub provided_capabilities: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub active_capabilities: Vec<String>,
    pub active_capability_providers: Vec<CapabilityProviderSelection>,
    pub active_resource_providers: Vec<String>,
    pub active_host_extensions: Vec<String>,
    pub active_plugin_backends: Vec<String>,
    pub active_codecs: Vec<String>,
    pub active_bridges: Vec<String>,
    pub active_scheduler_policies: Vec<String>,
    pub active_workflows: Vec<String>,
    pub permission_audit: Vec<PermissionAuditEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityProviderSelection {
    pub capability: String,
    pub provider_plugin_id: String,
    pub provider_version: Option<String>,
    pub surface_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionAuditEntry {
    pub plugin_id: String,
    pub permission_kind: String,
    pub permission: String,
    pub granted: bool,
    pub provider_capability: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractSurfaceKind {
    Runner,
    TaskProtocol,
    Schema,
    ResourceSchema,
    ResourceProvider,
    HostExtension,
    PluginBackend,
    Codec,
    Bridge,
    SchedulerPolicy,
    Workflow,
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
    pub plugin_deployments: BTreeMap<String, PluginDeploymentKind>,
    pub capability_graph: RuntimeCapabilityGraph,
    pub contract_surfaces: Vec<ContractSurface>,
}

pub type RuntimeLock = RuntimeLoadPlan;
