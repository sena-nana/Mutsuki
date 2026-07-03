use serde::{Deserialize, Serialize};

use crate::{ProtocolId, plugin::PluginDeploymentKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostExtensionKind {
    PluginBackend,
    Bridge,
    Codec,
    TraceSink,
    SchedulerPolicy,
    PermissionPolicy,
    ResourcePlanGateway,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionDescriptor {
    pub extension_id: String,
    pub kind: HostExtensionKind,
    pub supported_deployments: Vec<PluginDeploymentKind>,
    pub reload_policy: String,
    pub drain_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginBackendDescriptor {
    pub backend_id: String,
    pub deployment_kind: PluginDeploymentKind,
    pub task_client_protocol: String,
    pub resource_client_protocol: String,
    pub codec_id: Option<String>,
    pub bridge_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecDescriptor {
    pub codec_id: String,
    pub media_type: String,
    pub version: String,
    pub connection_scoped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeDescriptor {
    pub bridge_id: String,
    pub deployment_kind: PluginDeploymentKind,
    pub codec_ids: Vec<String>,
    pub drain_policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerPolicyDescriptor {
    pub policy_id: String,
    pub version: String,
    pub decision_scope: String,
}

/// Experimental workflow plugin binding descriptor. Workflow instance state
/// must live in an external resource; CoreRuntime does not own workflow state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDescriptor {
    pub workflow_id: String,
    pub state_resource_kind: String,
    pub runner_protocol_id: ProtocolId,
    pub reload_policy: String,
}
