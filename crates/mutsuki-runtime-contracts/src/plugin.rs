use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::extension::{
    BridgeDescriptor, CodecDescriptor, HostExtensionDescriptor, PluginBackendDescriptor,
    SchedulerPolicyDescriptor, WorkflowDescriptor,
};
use crate::{
    BindingId, ObservabilityProfile, ProtocolId, ResourceTypeDescriptor, RunnerDescriptor,
    ScalarValue, SurfaceId,
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfileMode {
    #[default]
    FullDev,
    ExtensibleRuntime,
    BuiltinOnly,
    LockedBuiltin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionArtifact {
    /// Package-relative path resolved and validated by the product Host.
    pub path: String,
    pub sha256: String,
    #[serde(default)]
    pub executable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginArtifact {
    pub artifact_type: ArtifactType,
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub companion_artifacts: Vec<CompanionArtifact>,
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

/// Versioned, owner-defined business metadata published by a plugin.
///
/// Core and generic Hosts carry and compare this descriptor, but never interpret `payload`.
/// Domain owners use `extension_id` and `version` to decode it into their typed contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginExtensionDescriptor {
    pub extension_id: String,
    pub version: u32,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginProvides {
    /// Owner-defined load-plan capabilities that do not imply a runner, protocol, resource,
    /// or Host backend surface.
    ///
    /// Values use the same fully-qualified capability syntax as `PluginManifest::requires`.
    /// They are validated and activated by the Host resolver, but remain opaque to Core.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<PluginExtensionDescriptor>,
    pub runners: Vec<RunnerDescriptor>,
    pub protocols: Vec<ProtocolDescriptor>,
    /// Runtime semantic class for each task protocol.
    ///
    /// Empty maps are accepted only as legacy manifest input and are
    /// normalized by the Host resolver before a RuntimeLoadPlan is emitted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub protocol_classes: BTreeMap<String, ProtocolClass>,
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

/// A typed dependency on a public surface owned by another plugin or the Host.
///
/// Requirements are resolved before registry freeze. Implementations must not
/// infer dependencies from plugin ids, load order, or protocol-name prefixes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SurfaceRequirement {
    pub kind: ContractSurfaceKind,
    pub surface_id: SurfaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl SurfaceRequirement {
    pub fn new(kind: ContractSurfaceKind, surface_id: impl Into<SurfaceId>) -> Self {
        Self {
            kind,
            surface_id: surface_id.into(),
            version: None,
        }
    }

    pub fn task_protocol(protocol_id: impl Into<SurfaceId>) -> Self {
        Self::new(ContractSurfaceKind::TaskProtocol, protocol_id)
    }

    pub fn service(service_id: impl Into<SurfaceId>) -> Self {
        Self::new(ContractSurfaceKind::Service, service_id)
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn capability_key(&self) -> String {
        format!("{}:{}", self.kind.capability_prefix(), self.surface_id)
    }

    pub fn display_key(&self) -> String {
        match &self.version {
            Some(version) => format!("{}@{version}", self.capability_key()),
            None => self.capability_key(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolClass {
    #[default]
    Domain,
    Effect,
    Core,
    Control,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<SurfaceRequirement>,
    pub permissions: PermissionGrant,
    pub lifecycle: LifecyclePolicy,
    pub metadata: BTreeMap<String, ScalarValue>,
}

/// Deployment-neutral capability surface used to compare multiple artifacts of the same plugin.
///
/// Artifact identity, lifecycle policy and the Host transport descriptors are deliberately
/// excluded. Those fields may differ between builtin, ABI and process deployments without
/// changing the business contract exposed by the plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginBusinessSurface {
    pub plugin_id: String,
    pub api_version: String,
    pub provides: PluginProvides,
    pub requires: Vec<SurfaceRequirement>,
    pub permissions: PermissionGrant,
}

impl PluginManifest {
    pub fn business_surface(&self) -> PluginBusinessSurface {
        let mut provides = self.provides.clone();
        provides.host_extensions.clear();
        provides.plugin_backends.clear();
        provides.codecs.clear();
        provides.bridges.clear();
        PluginBusinessSurface {
            plugin_id: self.plugin_id.clone(),
            api_version: self.api_version.clone(),
            provides,
            requires: self.requires.clone(),
            permissions: self.permissions.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeProfile {
    pub profile_id: String,
    pub mode: RuntimeProfileMode,
    pub enabled_plugins: Vec<String>,
    pub bindings: BTreeMap<String, String>,
    /// Explicit public-surface provider selection. Keys use the canonical
    /// `<kind>:<surface-id>` form and values are provider plugin ids.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub surface_bindings: BTreeMap<String, String>,
    pub plugin_deployments: BTreeMap<String, PluginDeploymentKind>,
    pub observability: ObservabilityProfile,
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    PluginExtension,
    StateSchema,
    Lifecycle,
    Permission,
    Capability,
    Service,
}

impl ContractSurfaceKind {
    pub fn capability_prefix(&self) -> &'static str {
        match self {
            Self::Runner => "runner",
            Self::TaskProtocol => "task_protocol",
            Self::Schema => "schema",
            Self::ResourceSchema => "resource_schema",
            Self::ResourceProvider => "resource_provider",
            Self::HostExtension => "host_extension",
            Self::PluginBackend => "plugin_backend",
            Self::Codec => "codec",
            Self::Bridge => "bridge",
            Self::SchedulerPolicy => "scheduler_policy",
            Self::Workflow => "workflow",
            Self::Effect => "effect",
            Self::Stream => "stream",
            Self::Subscription => "subscription",
            Self::Timer => "timer",
            Self::Protocol => "protocol",
            Self::HandlerBinding => "handler_binding",
            Self::PluginExtension => "plugin_extension",
            Self::StateSchema => "state_schema",
            Self::Lifecycle => "lifecycle",
            Self::Permission => "permission",
            Self::Capability => "capability",
            Self::Service => "service",
        }
    }
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
    pub observability: ObservabilityProfile,
    pub capability_graph: RuntimeCapabilityGraph,
    pub contract_surfaces: Vec<ContractSurface>,
}

pub type RuntimeLock = RuntimeLoadPlan;
