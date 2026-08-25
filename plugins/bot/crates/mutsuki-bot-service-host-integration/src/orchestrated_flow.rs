use mutsuki_bot_interaction::{
    BOT_INTERACTION_CREATE_NODE_TYPE, BOT_INTERACTION_MATCH_NODE_TYPE,
    DEFAULT_INTERACTION_WAITER_TIMEOUT_MS,
};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE, BotFlowDocument, BotFlowEdge,
    BotFlowEdgeKind, BotFlowNode, BotFlowNodePosition, BotFlowSourceSelector, BotFlowTypeRef,
};
use mutsuki_plugin_bot_agent::{BOT_AGENT_NODE_BIND_PROFILE, BOT_AGENT_NODE_SUBMIT};
use mutsuki_plugin_bot_command::{BOT_COMMAND_MATCH_NODE_TYPE_ID, BOT_COMMAND_REPLY_NODE_TYPE_ID};
use mutsuki_plugin_bot_conversation_context::{
    BOT_CONVERSATION_ATTACH_ICL_NODE_TYPE, BOT_CONVERSATION_ATTACH_IDENTIFIERS_NODE_TYPE,
    BOT_CONVERSATION_RECORD_ICL_NODE_TYPE,
};
use mutsuki_plugin_bot_persona::{BOT_PERSONA_ATTACH_NODE_TYPE, BOT_PERSONA_COMMAND_NODE_TYPE};
use mutsuki_plugin_bot_reply::{
    BOT_REPLY_MENTION_NODE_TYPE, BOT_REPLY_QUOTE_NODE_TYPE, BOT_REPLY_SEGMENT_NODE_TYPE,
};
use serde_json::json;

const QQ_FORWARD_FOLD_NODE_TYPE: &str = "mutsuki.bot.qq.reply.forward_fold";
pub const QQ_AI_PRESENTATION_FAILURE_TEXT: &str = "刚才没能发出回复，请稍后再试。";

/// First-party example graph. Source fans out to `/persona` matching and `record-icl-listen`.
/// Empty-mention Create opens a waiter; the next ingress rematches. Bare @ never edges to Agent.
/// Submit is record-icl → attach-icl → identifiers → attach-bound-persona → bind-profile →
/// agent → quote → mention-reply → segment → QQ `forward_fold` → delivery.
#[must_use]
pub fn qq_ai_orchestrated_flow() -> BotFlowDocument {
    qq_ai_orchestrated_flow_with_source(
        "mutsuki.bot.qq.message.created",
        Some(BotFlowSourceSelector {
            protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
            event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
        }),
    )
}

#[must_use]
pub fn qq_ai_orchestrated_flow_with_source(
    source_node_type: &str,
    source: Option<BotFlowSourceSelector>,
) -> BotFlowDocument {
    BotFlowDocument {
        flow_id: "qq.ai.orchestrated".into(),
        name: "QQ AI 可编排对话".into(),
        nodes: vec![
            flow_node("source", source_node_type, json!({}), source),
            flow_node(
                "persona-command",
                BOT_COMMAND_MATCH_NODE_TYPE_ID,
                json!({
                    "prefixes": ["/"],
                    "path": ["persona"],
                    "aliases": [],
                    "arguments": [{
                        "name": "id",
                        "kind": "string",
                        "optional": true,
                        "variadic": false
                    }],
                    "case_sensitive": false
                }),
                None,
            ),
            flow_node("persona", BOT_PERSONA_COMMAND_NODE_TYPE, json!({}), None),
            flow_node("qq-send", "mutsuki.bot.qq.send", json!({}), None),
            flow_node(
                "empty-mention",
                "mutsuki.bot.match.empty_mention",
                json!({}),
                None,
            ),
            flow_node(
                "interaction-create",
                BOT_INTERACTION_CREATE_NODE_TYPE,
                json!({"timeout_ms": DEFAULT_INTERACTION_WAITER_TIMEOUT_MS}),
                None,
            ),
            flow_node(
                "interaction",
                BOT_INTERACTION_MATCH_NODE_TYPE,
                json!({}),
                None,
            ),
            flow_node("mention", "mutsuki.bot.match.mention", json!({}), None),
            flow_node(
                "record-icl",
                BOT_CONVERSATION_RECORD_ICL_NODE_TYPE,
                json!({"max_count": 20}),
                None,
            ),
            flow_node(
                "record-icl-listen",
                BOT_CONVERSATION_RECORD_ICL_NODE_TYPE,
                json!({"max_count": 20}),
                None,
            ),
            flow_node(
                "attach-icl",
                BOT_CONVERSATION_ATTACH_ICL_NODE_TYPE,
                json!({"max_count": 20}),
                None,
            ),
            flow_node(
                "identifiers",
                BOT_CONVERSATION_ATTACH_IDENTIFIERS_NODE_TYPE,
                json!({}),
                None,
            ),
            flow_node(
                "attach-bound-persona",
                BOT_PERSONA_ATTACH_NODE_TYPE,
                json!({}),
                None,
            ),
            flow_node(
                "bind-profile",
                BOT_AGENT_NODE_BIND_PROFILE,
                json!({"profile_id": "qq-assistant"}),
                None,
            ),
            flow_node("agent", BOT_AGENT_NODE_SUBMIT, json!({}), None),
            flow_node("quote", BOT_REPLY_QUOTE_NODE_TYPE, json!({}), None),
            flow_node(
                "mention-reply",
                BOT_REPLY_MENTION_NODE_TYPE,
                json!({}),
                None,
            ),
            flow_node("segment", BOT_REPLY_SEGMENT_NODE_TYPE, json!({}), None),
            flow_node("forward-fold", QQ_FORWARD_FOLD_NODE_TYPE, json!({}), None),
            flow_node("delivery", "mutsuki.bot.delivery.reply", json!({}), None),
            flow_node(
                "present-fail",
                BOT_COMMAND_REPLY_NODE_TYPE_ID,
                json!({
                    "text": QQ_AI_PRESENTATION_FAILURE_TEXT,
                    "reply": true
                }),
                None,
            ),
        ],
        edges: vec![
            flow_edge(
                "source-persona-cmd",
                "source",
                "event",
                "persona-command",
                "event",
            ),
            flow_edge(
                "source-listen",
                "source",
                "event",
                "record-icl-listen",
                "input",
            ),
            flow_edge(
                "persona-cmd-handler",
                "persona-command",
                "matched",
                "persona",
                "command",
            ),
            flow_edge(
                "persona-message-send",
                "persona",
                "message",
                "qq-send",
                "input",
            ),
            flow_edge(
                "cmd-unmatched-empty",
                "persona-command",
                "unmatched",
                "empty-mention",
                "event",
            ),
            flow_edge(
                "empty-matched-create",
                "empty-mention",
                "matched",
                "interaction-create",
                "event",
            ),
            flow_edge(
                "empty-unmatched-interaction",
                "empty-mention",
                "unmatched",
                "interaction",
                "event",
            ),
            flow_edge(
                "interaction-unmatched-mention",
                "interaction",
                "unmatched",
                "mention",
                "event",
            ),
            flow_edge(
                "interaction-record",
                "interaction",
                "matched",
                "record-icl",
                "input",
            ),
            flow_edge(
                "mention-record",
                "mention",
                "matched",
                "record-icl",
                "input",
            ),
            flow_edge(
                "record-attach",
                "record-icl",
                "output",
                "attach-icl",
                "input",
            ),
            flow_edge("icl-ids", "attach-icl", "output", "identifiers", "input"),
            flow_edge(
                "ids-persona",
                "identifiers",
                "output",
                "attach-bound-persona",
                "input",
            ),
            flow_edge(
                "persona-profile",
                "attach-bound-persona",
                "output",
                "bind-profile",
                "input",
            ),
            flow_edge("profile-agent", "bind-profile", "output", "agent", "input"),
            flow_edge("agent-quote", "agent", "reply", "quote", "reply"),
            flow_edge("quote-mention", "quote", "reply", "mention-reply", "reply"),
            flow_edge(
                "mention-segment",
                "mention-reply",
                "reply",
                "segment",
                "reply",
            ),
            flow_edge("segment-fold", "segment", "reply", "forward-fold", "reply"),
            flow_edge(
                "fold-delivery",
                "forward-fold",
                "reply",
                "delivery",
                "reply",
            ),
            flow_error_edge("agent-fail", "agent", "present-fail"),
            flow_error_edge("quote-fail", "quote", "present-fail"),
            flow_error_edge("mention-reply-fail", "mention-reply", "present-fail"),
            flow_error_edge("segment-fail", "segment", "present-fail"),
            flow_error_edge("fold-fail", "forward-fold", "present-fail"),
            flow_error_edge("delivery-fail", "delivery", "present-fail"),
            flow_edge(
                "present-fail-send",
                "present-fail",
                "message",
                "qq-send",
                "input",
            ),
        ],
    }
}

fn flow_node(
    node_id: &str,
    node_type_id: &str,
    config: serde_json::Value,
    source: Option<BotFlowSourceSelector>,
) -> BotFlowNode {
    BotFlowNode {
        node_id: node_id.into(),
        node_type_id: node_type_id.into(),
        node_type_version: 1,
        config,
        source,
        position: BotFlowNodePosition::default(),
    }
}

fn flow_edge(
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

fn flow_error_edge(edge_id: &str, from_node_id: &str, to_node_id: &str) -> BotFlowEdge {
    BotFlowEdge {
        edge_id: edge_id.into(),
        from_node_id: from_node_id.into(),
        from_port_id: "error".into(),
        to_node_id: to_node_id.into(),
        to_port_id: "error".into(),
        kind: BotFlowEdgeKind::Error,
    }
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_delivery::bot_reply_delivery_manifest;
    use mutsuki_bot_flow::{BotNodeCatalog, validate_flow};
    use mutsuki_bot_interaction::bot_interaction_manifest;
    use mutsuki_plugin_bot_adapter_qqbot::qqbot_adapter_manifest;
    use mutsuki_plugin_bot_agent::bot_agent_bridge_manifest;
    use mutsuki_plugin_bot_command::bot_command_manifest;
    use mutsuki_plugin_bot_conversation_context::bot_conversation_context_manifest;
    use mutsuki_plugin_bot_event_router::flow_router_manifest;
    use mutsuki_plugin_bot_persona::bot_persona_manifest;
    use mutsuki_plugin_bot_reply::bot_reply_manifest;

    use super::*;

    fn first_party_catalog() -> BotNodeCatalog {
        BotNodeCatalog::from_manifests(&[
            qqbot_adapter_manifest(1, false),
            flow_router_manifest(),
            bot_command_manifest(1),
            bot_conversation_context_manifest(),
            bot_agent_bridge_manifest(),
            bot_reply_manifest(),
            bot_reply_delivery_manifest(),
            bot_persona_manifest(),
            bot_interaction_manifest(),
        ])
        .expect("first-party catalogs merge")
    }

    #[test]
    fn orchestrated_flow_validates_against_published_node_catalog() {
        let result = validate_flow(&qq_ai_orchestrated_flow(), &first_party_catalog());
        assert!(result.valid, "{:#?}", result.issues);
        let flow = qq_ai_orchestrated_flow();
        assert!(
            flow.nodes.iter().all(|node| node.node_id != "probability"
                && node.node_id != "attach-bound-persona-command")
        );
        assert!(
            flow.edges
                .iter()
                .any(|edge| edge.edge_id == "empty-matched-create")
        );
        assert!(
            flow.edges
                .iter()
                .any(|edge| edge.from_node_id == "empty-mention"
                    && edge.from_port_id == "matched"
                    && edge.to_node_id == "interaction-create")
        );
        assert!(
            flow.edges
                .iter()
                .any(|edge| edge.from_node_id == "empty-mention"
                    && edge.from_port_id == "unmatched"
                    && edge.to_node_id == "interaction")
        );
        assert!(
            flow.edges
                .iter()
                .any(|edge| edge.from_node_id == "interaction"
                    && edge.from_port_id == "unmatched"
                    && edge.to_node_id == "mention")
        );
        assert!(
            flow.edges
                .iter()
                .any(|edge| edge.from_node_id == "interaction"
                    && edge.from_port_id == "matched"
                    && edge.to_node_id == "record-icl")
        );
        assert!(flow.edges.iter().any(|edge| edge.from_node_id == "mention"
            && edge.from_port_id == "matched"
            && edge.to_node_id == "record-icl"));
        assert!(
            !flow
                .edges
                .iter()
                .any(|edge| edge.from_node_id == "empty-mention"
                    && edge.from_port_id == "matched"
                    && edge.to_node_id == "agent")
        );
        assert!(flow.edges.iter().any(|edge| edge.edge_id == "source-listen"
            && edge.from_node_id == "source"
            && edge.to_node_id == "record-icl-listen"));
        assert!(
            !flow
                .edges
                .iter()
                .any(|edge| edge.edge_id == "mention-listen")
        );
        assert!(
            flow.edges
                .iter()
                .any(|edge| edge.edge_id == "fold-delivery")
        );
        for edge_id in [
            "agent-fail",
            "quote-fail",
            "mention-reply-fail",
            "segment-fail",
            "fold-fail",
            "delivery-fail",
            "present-fail-send",
        ] {
            assert!(
                flow.edges.iter().any(|edge| edge.edge_id == edge_id),
                "missing {edge_id}"
            );
        }
        assert!(flow.edges.iter().all(|edge| {
            !matches!(edge.kind, BotFlowEdgeKind::Error)
                || (edge.from_port_id == "error" && edge.to_node_id == "present-fail")
        }));
    }

    #[test]
    fn unpublished_node_cannot_enter_orchestrated_graph() {
        let mut flow = qq_ai_orchestrated_flow();
        flow.nodes.push(flow_node(
            "unknown",
            "mutsuki.bot.flow.not_in_catalog",
            json!({}),
            None,
        ));
        let result = validate_flow(&flow, &first_party_catalog());
        assert!(!result.valid);
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.code == "flow.node.unavailable")
        );
    }
}
