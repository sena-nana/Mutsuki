use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{PluginExtensionDescriptor, RuntimeError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BotAccountRef, BotExtMap, BotTarget, BotUser};

pub const BOT_FLOW_NODE_EXTENSION_ID: &str = "mutsuki.bot.flow.nodes";
pub const BOT_FLOW_NODE_EXTENSION_VERSION: u32 = 1;
pub const BOT_FLOW_EVENT_TYPE: &str = "mutsuki.bot.flow.event";
pub const BOT_FLOW_ERROR_TYPE: &str = "mutsuki.bot.flow.error";
pub const BOT_FLOW_BOT_EVENT_TYPE: &str = "mutsuki.bot.event";
pub const BOT_FLOW_MESSAGE_EVENT_TYPE: &str = "mutsuki.bot.event.message";
pub const BOT_FLOW_MESSAGE_DELETED_EVENT_TYPE: &str = "mutsuki.bot.event.message_deleted";
pub const BOT_FLOW_REACTION_EVENT_TYPE: &str = "mutsuki.bot.event.reaction";
pub const BOT_FLOW_MEMBER_EVENT_TYPE: &str = "mutsuki.bot.event.member";
pub const BOT_FLOW_LIFECYCLE_EVENT_TYPE: &str = "mutsuki.bot.event.lifecycle";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BotFlowTypeRef {
    pub type_id: String,
    pub version: u32,
}

impl BotFlowTypeRef {
    pub fn new(type_id: impl Into<String>, version: u32) -> Self {
        Self {
            type_id: type_id.into(),
            version,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowPayload {
    pub event_type: BotFlowTypeRef,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<BotAccountRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<BotTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<BotUser>,
    #[serde(default, skip_serializing_if = "BotExtMap::is_empty")]
    pub ext: BotExtMap,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowEventEnvelope {
    pub event_id: String,
    pub protocol_id: String,
    pub payload: BotFlowPayload,
    pub context: BotFlowContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotNodeRole {
    Source,
    Match,
    Processor,
    Sink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotNodePortDirection {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotNodePortDescriptor {
    pub port_id: String,
    pub title: String,
    pub direction: BotNodePortDirection,
    pub event_type: BotFlowTypeRef,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotNodeBinding {
    pub binding_id: String,
    pub protocol_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotNodeDescriptor {
    pub node_type_id: String,
    pub version: u32,
    pub title: String,
    pub category: String,
    pub role: BotNodeRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<BotNodeBinding>,
    pub ports: Vec<BotNodePortDescriptor>,
    pub config_schema: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotNodeCatalogFragment {
    pub nodes: Vec<BotNodeDescriptor>,
}

impl BotNodeCatalogFragment {
    pub fn into_plugin_extension(self) -> Result<PluginExtensionDescriptor, serde_json::Error> {
        Ok(PluginExtensionDescriptor {
            extension_id: BOT_FLOW_NODE_EXTENSION_ID.into(),
            version: BOT_FLOW_NODE_EXTENSION_VERSION,
            projection: mutsuki_runtime_contracts::ExtensionProjection::Universal,
            payload: serde_json::to_value(self)?,
        })
    }

    pub fn from_plugin_extension(
        extension: &PluginExtensionDescriptor,
    ) -> Result<Option<Self>, serde_json::Error> {
        if extension.extension_id != BOT_FLOW_NODE_EXTENSION_ID
            || extension.version != BOT_FLOW_NODE_EXTENSION_VERSION
        {
            return Ok(None);
        }
        serde_json::from_value(extension.payload.clone()).map(Some)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotFlowSourceSelector {
    pub protocol_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<BotFlowTypeRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowNode {
    pub node_id: String,
    pub node_type_id: String,
    pub node_type_version: u32,
    #[serde(default)]
    pub config: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<BotFlowSourceSelector>,
    #[serde(default)]
    pub position: BotFlowNodePosition,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BotFlowNodePosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotFlowEdgeKind {
    Event,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotFlowEdge {
    pub edge_id: String,
    pub from_node_id: String,
    pub from_port_id: String,
    pub to_node_id: String,
    pub to_port_id: String,
    #[serde(default = "default_edge_kind")]
    pub kind: BotFlowEdgeKind,
}

fn default_edge_kind() -> BotFlowEdgeKind {
    BotFlowEdgeKind::Event
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowDocument {
    pub flow_id: String,
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub nodes: Vec<BotFlowNode>,
    pub edges: Vec<BotFlowEdge>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowSnapshot {
    pub revision: u64,
    pub flows: Vec<BotFlowDocument>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotFlowValidationSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotFlowValidationIssue {
    pub code: String,
    pub message: String,
    pub severity: BotFlowValidationSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotFlowValidationResult {
    pub valid: bool,
    pub issues: Vec<BotFlowValidationIssue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotNodeInvocation {
    pub flow_id: String,
    pub graph_revision: u64,
    pub execution_id: String,
    pub node_id: String,
    pub input_port_id: String,
    pub config: Value,
    pub input: BotFlowEventEnvelope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotNodeOutput {
    pub port_id: String,
    pub event: BotFlowEventEnvelope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotNodeResult {
    #[serde(default)]
    pub outputs: Vec<BotNodeOutput>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowNodeExecution {
    pub graph_revision: u64,
    /// The immutable flow is pinned into every task so applying a new config
    /// revision cannot change in-flight execution.
    pub flow: BotFlowDocument,
    pub execution_id: String,
    pub node_id: String,
    pub input_port_id: String,
    pub event: BotFlowEventEnvelope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowErrorEvent {
    pub failed_node_id: String,
    pub error: RuntimeError,
    pub input: BotFlowEventEnvelope,
}
