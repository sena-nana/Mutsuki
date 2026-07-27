use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentToolDescriptor, ResourceRef, ToolSideEffect};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPluginSurfaceKind {
    Tool,
    ContextProvider,
    Hook,
    Policy,
    Command,
    Service,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPluginStateKind {
    Stateless,
    Stateful,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentServiceDescriptor {
    pub service_id: String,
    pub version: String,
    pub request_schema: String,
    pub response_schema: String,
    pub state: AgentPluginStateKind,
    #[serde(default)]
    pub affinity: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentKitPluginDescriptor {
    pub plugin_id: String,
    pub generation: u64,
    #[serde(default)]
    pub tools: Vec<AgentToolDescriptor>,
    #[serde(default)]
    pub context_providers: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub policies: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub services: Vec<AgentServiceDescriptor>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub required_services: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextProviderRequest {
    pub session_id: String,
    pub turn_id: String,
    pub provider_id: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextProviderResult {
    pub provider_id: String,
    pub summary: String,
    #[serde(default)]
    pub details: Option<ResourceRef>,
    pub estimated_tokens: u64,
    #[serde(default)]
    pub estimated_bytes: u64,
    pub priority: i32,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub session_id: String,
    pub turn_id: String,
    pub action_id: String,
    pub tool: String,
    pub side_effect: ToolSideEffect,
    pub summary: String,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionKind {
    Approved,
    Rejected,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDecision {
    pub session_id: String,
    pub turn_id: String,
    pub action_id: String,
    pub version: u64,
    pub decision: PermissionDecisionKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentCommandRequest {
    pub command_id: String,
    pub session_id: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentCommandResult {
    pub command_id: String,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub output_ref: Option<ResourceRef>,
}
