//! Plugin surface for Bot delivery: manifests, catalog, and runners. Domain services live in
//! `mutsuki-bot-delivery`.

#![allow(clippy::must_use_candidate)]

mod runners;

use mutsuki_bot_delivery::BOT_SCHEDULED_DELIVERY_PROTOCOL_ID;
use mutsuki_bot_protocol::{
    BOT_ACTIVE_DELIVERY_PROTOCOL_ID, BOT_REPLY_DELIVERY_PROTOCOL_ID, BotFlowTypeRef,
    BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodePortDescriptor,
    BotNodePortDirection, BotNodeRole,
};
use mutsuki_runtime_contracts::PluginManifest;
use mutsuki_runtime_sdk::{PluginBuilder, ProtocolDescriptorBuilder};

pub use runners::{
    BOT_DELIVERY_PLUGIN_ID, BOT_DELIVERY_RUNNER_ID, BOT_REPLY_DELIVERY_PLUGIN_ID,
    BOT_REPLY_DELIVERY_RUNNER_ID, BOT_SCHEDULED_DELIVERY_PLUGIN_ID,
    BOT_SCHEDULED_DELIVERY_RUNNER_ID, delivery_descriptor, delivery_runner,
    reply_delivery_descriptor, reply_delivery_runner, reply_delivery_runner_for,
    scheduled_delivery_descriptor, scheduled_delivery_runner,
};

#[must_use]
pub fn bot_scheduled_delivery_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_SCHEDULED_DELIVERY_PLUGIN_ID)
        .runner_descriptor(scheduled_delivery_descriptor())
        .protocol_handler(
            delivery_protocol_descriptor(
                BOT_SCHEDULED_DELIVERY_PROTOCOL_ID,
                &["execution_id", "summary"],
                &["delivery_id", "status"],
            ),
            BOT_SCHEDULED_DELIVERY_RUNNER_ID,
            "bot-scheduled-delivery",
        )
        .extension(
            scheduled_delivery_node_catalog()
                .into_plugin_extension()
                .expect("scheduled delivery node catalog serializes"),
        )
        .build()
        .manifest
}

#[must_use]
pub fn bot_delivery_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_DELIVERY_PLUGIN_ID)
        .runner_descriptor(delivery_descriptor())
        .protocol_handler(
            delivery_protocol_descriptor(
                BOT_ACTIVE_DELIVERY_PROTOCOL_ID,
                &["action"],
                &["delivery_id", "status"],
            ),
            BOT_DELIVERY_RUNNER_ID,
            "bot-delivery",
        )
        .build()
        .manifest
}

#[must_use]
pub fn bot_reply_delivery_manifest() -> PluginManifest {
    bot_reply_delivery_manifest_for(BOT_REPLY_DELIVERY_PLUGIN_ID, BOT_REPLY_DELIVERY_RUNNER_ID)
}

#[must_use]
pub fn bot_reply_delivery_manifest_for(plugin_id: &str, runner_id: &str) -> PluginManifest {
    PluginBuilder::new(plugin_id)
        .runner_descriptor(reply_delivery_descriptor(plugin_id, runner_id))
        .protocol_handler(
            delivery_protocol_descriptor(
                BOT_REPLY_DELIVERY_PROTOCOL_ID,
                &["action"],
                &["reply_id", "part_receipts"],
            ),
            runner_id,
            "bot-reply-delivery",
        )
        .extension(
            BotNodeCatalogFragment {
                nodes: vec![BotNodeDescriptor {
                    node_type_id: "mutsuki.bot.delivery.reply".into(),
                    version: 1,
                    title: "可靠回复投递".into(),
                    category: "投递".into(),
                    role: BotNodeRole::Sink,
                    binding: Some(BotNodeBinding {
                        binding_id: format!("binding:{BOT_REPLY_DELIVERY_PROTOCOL_ID}"),
                        protocol_id: BOT_REPLY_DELIVERY_PROTOCOL_ID.into(),
                        runner_hint: Some(runner_id.into()),
                    }),
                    ports: vec![BotNodePortDescriptor {
                        port_id: "reply".into(),
                        title: "回复".into(),
                        direction: BotNodePortDirection::Input,
                        event_type: BotFlowTypeRef::new("mutsuki.bot.delivery.reply", 1),
                        required: true,
                    }],
                    config_schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false
                    }),
                }],
            }
            .into_plugin_extension()
            .expect("delivery node catalog serializes"),
        )
        .build()
        .manifest
}

fn delivery_protocol_descriptor(
    protocol_id: &str,
    request_required: &[&str],
    response_required: &[&str],
) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(serde_json::json!({
            "type": "object",
            "required": request_required
        }))
        .output_schema(serde_json::json!({
            "type": "object",
            "required": response_required
        }))
        .error_schema(serde_json::json!({
            "type": "object",
            "required": ["code", "source", "route"]
        }))
        .build()
}

fn scheduled_delivery_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![BotNodeDescriptor {
            node_type_id: "mutsuki.bot.delivery.scheduled".into(),
            version: 1,
            title: "定时投递".into(),
            category: "投递".into(),
            role: BotNodeRole::Sink,
            binding: Some(BotNodeBinding {
                binding_id: format!("binding:{BOT_SCHEDULED_DELIVERY_PROTOCOL_ID}"),
                protocol_id: BOT_SCHEDULED_DELIVERY_PROTOCOL_ID.into(),
                runner_hint: Some(BOT_SCHEDULED_DELIVERY_RUNNER_ID.into()),
            }),
            ports: vec![BotNodePortDescriptor {
                port_id: "result".into(),
                title: "定时结果".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new("mutsuki.bot.delivery.scheduled", 1),
                required: true,
            }],
            config_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
        }],
    }
}
