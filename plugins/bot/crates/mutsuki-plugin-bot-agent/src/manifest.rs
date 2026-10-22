#[allow(clippy::wildcard_imports)]
use super::*;

#[must_use]
pub fn bot_agent_bridge_manifest() -> PluginManifest {
    let mut builder =
        PluginBuilder::new(BOT_AGENT_BRIDGE_PLUGIN_ID).runner_descriptor(agent_bridge_descriptor());
    for (protocol_id, binding_name) in [
        (BOT_AGENT_SUBMIT_PROTOCOL_ID, "bot-agent-submit"),
        (BOT_AGENT_CANCEL_PROTOCOL_ID, "bot-agent-cancel"),
        (BOT_AGENT_RESET_PROTOCOL_ID, "bot-agent-reset"),
        (BOT_AGENT_FORK_PROTOCOL_ID, "bot-agent-fork"),
        (BOT_AGENT_STATUS_PROTOCOL_ID, "bot-agent-status"),
        (BOT_AGENT_REGENERATE_PROTOCOL_ID, "bot-agent-regenerate"),
        (BOT_AGENT_BIND_PROFILE_PROTOCOL_ID, "bot-agent-bind-profile"),
    ] {
        builder = builder.protocol_handler(
            ProtocolDescriptorBuilder::new(protocol_id)
                .input_schema(serde_json::json!({
                    "type": "object",
                    "required": ["action", "event"]
                }))
                .output_schema(serde_json::json!({
                    "type": "object",
                    "required": ["binding", "turn_id", "outgoing", "resolved"]
                }))
                .error_schema(serde_json::json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            BOT_AGENT_BRIDGE_RUNNER_ID,
            binding_name,
        );
    }
    builder
        .extension(
            agent_node_catalog()
                .into_plugin_extension()
                .expect("Agent node catalog serializes"),
        )
        .build()
        .manifest
}

pub(crate) fn agent_node_catalog() -> BotNodeCatalogFragment {
    let definitions = [
        (
            BOT_AGENT_NODE_SUBMIT,
            "提交 Agent",
            BOT_AGENT_SUBMIT_PROTOCOL_ID,
            BOT_FLOW_BOT_EVENT_TYPE,
        ),
        (
            BOT_AGENT_NODE_CANCEL,
            "取消回复",
            BOT_AGENT_CANCEL_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_RESET,
            "重置会话",
            BOT_AGENT_RESET_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_FORK,
            "分叉会话",
            BOT_AGENT_FORK_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_STATUS,
            "会话状态",
            BOT_AGENT_STATUS_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_REGENERATE,
            "重新生成",
            BOT_AGENT_REGENERATE_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
    ];
    let mut nodes = definitions
        .into_iter()
        .map(
            |(node_type_id, title, protocol_id, input_type)| BotNodeDescriptor {
                node_type_id: node_type_id.into(),
                version: 1,
                title: title.into(),
                category: "Agent".into(),
                role: BotNodeRole::Processor,
                binding: Some(BotNodeBinding {
                    binding_id: format!("binding:{protocol_id}"),
                    protocol_id: protocol_id.into(),
                    runner_hint: Some(BOT_AGENT_BRIDGE_RUNNER_ID.into()),
                }),
                ports: vec![
                    BotNodePortDescriptor {
                        port_id: "input".into(),
                        title: "输入".into(),
                        direction: BotNodePortDirection::Input,
                        event_type: BotFlowTypeRef::new(input_type, 1),
                        required: true,
                    },
                    BotNodePortDescriptor {
                        port_id: "reply".into(),
                        title: "回复".into(),
                        direction: BotNodePortDirection::Output,
                        event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                        required: false,
                    },
                ],
                config_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            },
        )
        .collect::<Vec<_>>();
    nodes.push(BotNodeDescriptor {
        node_type_id: BOT_AGENT_NODE_BIND_PROFILE.into(),
        version: 1,
        title: "绑定人格".into(),
        category: "Agent".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{BOT_AGENT_BIND_PROFILE_PROTOCOL_ID}"),
            protocol_id: BOT_AGENT_BIND_PROFILE_PROTOCOL_ID.into(),
            runner_hint: Some(BOT_AGENT_BRIDGE_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "input".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "output".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
        ],
        config_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "profile_id": {"type": "string", "minLength": 1, "title": "Agent 配置文件"},
                "override": {"type": "boolean", "default": false, "title": "覆盖已有人格"}
            }
        }),
    });
    BotNodeCatalogFragment { nodes }
}
