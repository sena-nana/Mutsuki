use mutsuki_bot_flow::{BotFlowError, BotNodeCatalog, validate_flow};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE, BotFlowDocument, BotFlowEdge,
    BotFlowEdgeKind, BotFlowNode, BotFlowNodePosition, BotFlowSourceSelector, BotFlowTypeRef,
    BotFlowValidationResult,
};
use mutsuki_plugin_bot_adapter_qqbot::tasks::qqbot_adapter_manifest;
use mutsuki_plugin_bot_command::{BOT_COMMAND_MATCH_NODE_TYPE_ID, bot_command_manifest};
use mutsuki_plugin_bot_event_router::flow_router_manifest;
use mutsuki_runtime_contracts::PluginManifest;
use serde_json::{Value, json};

pub use bot_echo::{ECHO_PLUGIN_ID, ECHO_RUNNER_ID, echo_manifest, echo_runner};

/// The example is a graph document, not plugin-owned routing configuration. Import or recreate this
/// document in the Bot Flow editor and save it before QQ events can invoke the nodes.
#[must_use]
pub fn qqbot_echo_flow() -> BotFlowDocument {
    BotFlowDocument {
        flow_id: "example.qq.echo".into(),
        name: "QQ /echo".into(),
        nodes: vec![
            node(
                "source",
                mutsuki_plugin_bot_adapter_qqbot::tasks::QQ_NODE_MESSAGE_CREATED,
                json!({}),
                Some(BotFlowSourceSelector {
                    protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                    event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
                }),
                40.0,
                120.0,
            ),
            node(
                "command",
                BOT_COMMAND_MATCH_NODE_TYPE_ID,
                json!({
                    "prefixes": ["/"],
                    "path": ["echo"],
                    "aliases": [],
                    "arguments": [{"name": "text", "kind": "string", "optional": true, "variadic": true}],
                    "case_sensitive": false
                }),
                None,
                300.0,
                120.0,
            ),
            node("echo", "example.bot.echo", json!({}), None, 560.0, 120.0),
            node("send", "mutsuki.bot.qq.send", json!({}), None, 820.0, 120.0),
        ],
        edges: vec![
            edge("source-command", "source", "event", "command", "event"),
            edge("command-echo", "command", "matched", "echo", "command"),
            edge("echo-send", "echo", "message", "send", "input"),
        ],
    }
}

#[must_use]
pub fn qqbot_echo_and_ping_flow() -> BotFlowDocument {
    let mut flow = qqbot_echo_flow();
    flow.flow_id = "example.qq.commands".into();
    flow.name = "QQ echo/ping".into();
    flow.nodes.extend([
        node(
            "ping-command",
            BOT_COMMAND_MATCH_NODE_TYPE_ID,
            json!({
                "prefixes": ["/"],
                "path": ["ping"],
                "aliases": [],
                "arguments": [],
                "case_sensitive": false
            }),
            None,
            300.0,
            260.0,
        ),
        node("ping", "example.bot.ping", json!({}), None, 560.0, 260.0),
        node(
            "ping-send",
            "mutsuki.bot.qq.send",
            json!({}),
            None,
            820.0,
            260.0,
        ),
    ]);
    flow.edges.extend([
        edge("source-ping", "source", "event", "ping-command", "event"),
        edge(
            "ping-command-ping",
            "ping-command",
            "matched",
            "ping",
            "command",
        ),
        edge("ping-send", "ping", "message", "ping-send", "input"),
    ]);
    flow
}

#[must_use]
pub fn qqbot_echo_manifests() -> Vec<PluginManifest> {
    vec![
        qqbot_adapter_manifest(1, false),
        flow_router_manifest(),
        bot_command_manifest(1),
        echo_manifest(1),
    ]
}

pub fn validate_example_flow() -> Result<BotFlowValidationResult, BotFlowError> {
    let manifests = qqbot_echo_manifests();
    let catalog = BotNodeCatalog::from_manifests(&manifests)?;
    Ok(validate_flow(&qqbot_echo_flow(), &catalog))
}

#[must_use]
pub fn example_flow_config_json() -> Value {
    json!({
        "flow": qqbot_echo_flow()
    })
}

fn node(
    node_id: &str,
    node_type_id: &str,
    config: Value,
    source: Option<BotFlowSourceSelector>,
    x: f64,
    y: f64,
) -> BotFlowNode {
    BotFlowNode {
        node_id: node_id.into(),
        node_type_id: node_type_id.into(),
        node_type_version: 1,
        config,
        source,
        position: BotFlowNodePosition { x, y },
    }
}

fn edge(
    edge_id: &str,
    from_node_id: &str,
    from_port_id: &str,
    to_node_id: &str,
    to_port_id: &str,
) -> BotFlowEdge {
    BotFlowEdge {
        edge_id: edge_id.into(),
        from_node_id: from_node_id.into(),
        from_port_id: from_port_id.into(),
        to_node_id: to_node_id.into(),
        to_port_id: to_port_id.into(),
        kind: BotFlowEdgeKind::Event,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{BOT_FLOW_NODE_EXTENSION_ID, BotNodeCatalogFragment};
    use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode};
    use mutsuki_runtime_host::resolve_load_plan;
    use std::collections::BTreeMap;

    #[test]
    fn example_flow_is_valid_against_the_real_plugin_catalog() {
        let validation = validate_example_flow().unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
    }

    #[test]
    fn manifests_only_publish_node_catalogs_not_command_names() {
        for manifest in qqbot_echo_manifests() {
            let encoded = serde_json::to_value(&manifest).unwrap();
            let decoded: PluginManifest = serde_json::from_value(encoded).unwrap();
            for extension in decoded
                .provides
                .extensions
                .iter()
                .filter(|extension| extension.extension_id == BOT_FLOW_NODE_EXTENSION_ID)
            {
                let fragment = BotNodeCatalogFragment::from_plugin_extension(extension)
                    .unwrap()
                    .unwrap();
                assert!(!fragment.nodes.is_empty());
            }
        }
        let profile = RuntimeProfile {
            profile_id: "qqbot-echo".into(),
            mode: RuntimeProfileMode::FullDev,
            enabled_plugins: qqbot_echo_manifests()
                .iter()
                .map(|manifest| manifest.plugin_id.clone())
                .collect(),
            bindings: BTreeMap::new(),
            surface_bindings: BTreeMap::new(),
            supported_extensions: Vec::new(),
            plugin_deployments: BTreeMap::new(),
            observability: Default::default(),
            allow_dynamic_registration: false,
            allow_hot_reload: false,
        };
        resolve_load_plan(&qqbot_echo_manifests(), &profile).unwrap();
    }
}
