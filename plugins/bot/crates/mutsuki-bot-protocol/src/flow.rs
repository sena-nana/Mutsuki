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
pub const BOT_FLOW_PLATFORM_EVENT_TYPE: &str = "mutsuki.bot.event.platform";
pub const BOT_FLOW_DELIVERY_REPLY_TYPE: &str = "mutsuki.bot.delivery.reply";

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

    #[must_use]
    pub fn assigns_to(&self, target: &Self) -> bool {
        if self == target {
            return true;
        }
        self.version == target.version
            && target.type_id == BOT_FLOW_BOT_EVENT_TYPE
            && self.type_id.starts_with("mutsuki.bot.event.")
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

const BOT_FLOW_DEFAULT_ID: &str = "default";
const BOT_FLOW_DEFAULT_NAME: &str = "流程";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowDocument {
    #[serde(default = "default_flow_id")]
    pub flow_id: String,
    #[serde(default = "default_flow_name")]
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<BotFlowNode>,
    #[serde(default)]
    pub edges: Vec<BotFlowEdge>,
}

impl Default for BotFlowDocument {
    fn default() -> Self {
        Self {
            flow_id: default_flow_id(),
            name: default_flow_name(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }
}

fn default_flow_id() -> String {
    BOT_FLOW_DEFAULT_ID.into()
}

fn default_flow_name() -> String {
    BOT_FLOW_DEFAULT_NAME.into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotFlowSnapshot {
    pub revision: u64,
    pub flow: BotFlowDocument,
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

/// Port-level connection state of one node instance inside the pinned flow
/// graph. The router derives it from the immutable graph revision of the
/// execution, so a plugin can learn whether it is wired without owning the
/// document. Port lists are sorted and deduplicated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotNodeWiring {
    /// Input ports that at least one event edge targets.
    #[serde(default)]
    pub wired_inputs: Vec<String>,
    /// Output ports with at least one outgoing event edge.
    #[serde(default)]
    pub wired_outputs: Vec<String>,
    /// Whether at least one error edge leaves this node.
    #[serde(default)]
    pub error_wired: bool,
}

impl BotNodeWiring {
    /// The node instance has any inbound or outbound connection in the graph.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.wired_inputs.is_empty() || self.has_downstream()
    }

    /// The node can hand an event to a downstream node through an event or
    /// error edge; an unwired Source chain freezes its business by design.
    #[must_use]
    pub fn has_downstream(&self) -> bool {
        !self.wired_outputs.is_empty() || self.error_wired
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotNodeInvocation {
    pub flow_id: String,
    pub graph_revision: u64,
    pub execution_id: String,
    pub node_id: String,
    pub input_port_id: String,
    #[serde(default)]
    pub wiring: BotNodeWiring,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_invocation() -> BotNodeInvocation {
        BotNodeInvocation {
            flow_id: "default".into(),
            graph_revision: 3,
            execution_id: "exec".into(),
            node_id: "card".into(),
            input_port_id: "event".into(),
            wiring: BotNodeWiring {
                wired_inputs: vec!["event".into()],
                wired_outputs: vec!["message".into()],
                error_wired: true,
            },
            config: serde_json::json!({}),
            input: BotFlowEventEnvelope {
                event_id: "e1".into(),
                protocol_id: "mutsuki.bot.event/ingest@1".into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
                    value: serde_json::json!({}),
                },
                context: BotFlowContext {
                    bot: None,
                    target: None,
                    actor: None,
                    ext: BotExtMap::new(),
                },
                trace_id: None,
                correlation_id: None,
            },
        }
    }

    #[test]
    fn invocation_wiring_round_trips_and_defaults_for_older_senders() {
        let invocation = sample_invocation();
        let encoded = serde_json::to_value(&invocation).unwrap();
        assert_eq!(
            serde_json::from_value::<BotNodeInvocation>(encoded).unwrap(),
            invocation
        );

        let mut legacy = serde_json::to_value(&invocation).unwrap();
        legacy.as_object_mut().unwrap().remove("wiring");
        let decoded = serde_json::from_value::<BotNodeInvocation>(legacy).unwrap();
        assert_eq!(decoded.wiring, BotNodeWiring::default());
        assert!(!decoded.wiring.is_connected());
    }

    #[test]
    fn wiring_summary_flags_distinguish_inbound_from_downstream() {
        let wiring = BotNodeWiring {
            wired_inputs: vec!["event".into()],
            ..BotNodeWiring::default()
        };
        assert!(wiring.is_connected());
        assert!(!wiring.has_downstream());

        let downstream = BotNodeWiring {
            error_wired: true,
            ..BotNodeWiring::default()
        };
        assert!(downstream.is_connected());
        assert!(downstream.has_downstream());
    }
}
