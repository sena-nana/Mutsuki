//! The Bilibili plugin manifest, runner descriptor and Flow node catalog.

use mutsuki_bot_protocol::{
    BotFlowTypeRef, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeRole,
};
use mutsuki_protocol_browser::SNAPSHOT;
use mutsuki_protocol_image::{CARD_RENDER, QR_RENDER};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, ProtocolClass, RunnerBatchCapability, RunnerContext,
    RunnerDescriptor, RunnerMode, RunnerPurity, RunnerSideEffect, RuntimeError, ScalarValue,
    WorkBatch,
};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, Runner, RunnerDescriptorBuilder, RuntimeResult,
};
use serde_json::json;

use crate::{
    BILIBILI_CARD_NODE_TYPE, BILIBILI_EVENT_TYPE, BILIBILI_NOTIFICATION_NODE_TYPE, BilibiliConfig,
    LINK_RESOLVE, MANAGEMENT_COMMAND, NOTIFY_CARD, PLUGIN_ID, POLL_DYNAMIC, POLL_LIVE, POLL_VIDEO,
    RUNNER_ID,
};

pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    manifest_with_surfaces(false, false)
}

pub fn manifest_for_config(config: &BilibiliConfig) -> mutsuki_runtime_contracts::PluginManifest {
    manifest_with_surfaces(config.management.enabled, config.risk_control.is_some())
}

/// Builds the manifest for the single (web-cookie) backend. `management_nodes`
/// only adds the subscription-management node descriptors: the management
/// command protocol and the QR renderer requirement are unconditional because
/// QR login does not depend on the management switch.
#[must_use]
pub fn manifest_with_surfaces(
    management_nodes: bool,
    risk_control_enabled: bool,
) -> mutsuki_runtime_contracts::PluginManifest {
    let mut builder = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestRunner {
            descriptor: runner_descriptor(risk_control_enabled),
        }))
        .protocol_handler(protocol(POLL_LIVE), RUNNER_ID, "orchestration")
        .protocol_handler(protocol(POLL_VIDEO), RUNNER_ID, "orchestration")
        .protocol_handler(protocol(NOTIFY_CARD), RUNNER_ID, "orchestration")
        .protocol_handler(protocol(POLL_DYNAMIC), RUNNER_ID, "orchestration")
        .protocol_handler(protocol(LINK_RESOLVE), RUNNER_ID, "orchestration")
        .protocol_handler(protocol(MANAGEMENT_COMMAND), RUNNER_ID, "orchestration");
    let mut nodes = Vec::new();
    nodes.push(BotNodeDescriptor {
        node_type_id: BILIBILI_NOTIFICATION_NODE_TYPE.into(),
        version: 1,
        title: "Bilibili 通知".into(),
        category: "Bilibili".into(),
        role: BotNodeRole::Source,
        binding: None,
        ports: vec![BotNodePortDescriptor {
            port_id: "event".into(),
            title: "事件".into(),
            direction: BotNodePortDirection::Output,
            event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
            required: false,
        }],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    });
    nodes.push(BotNodeDescriptor {
        node_type_id: BILIBILI_CARD_NODE_TYPE.into(),
        version: 1,
        title: "Bilibili 推送卡片".into(),
        category: "Bilibili".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{NOTIFY_CARD}"),
            protocol_id: NOTIFY_CARD.into(),
            runner_hint: Some(RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "通知".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "message".into(),
                title: "发送消息".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(
                    mutsuki_bot_protocol::BOT_FLOW_MESSAGE_SEND_TYPE,
                    1,
                ),
                required: false,
            },
        ],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    });
    nodes.push(BotNodeDescriptor {
        node_type_id: "mutsuki.bot.bilibili.resolve".into(),
        version: 1,
        title: "Bilibili 链接".into(),
        category: "链接".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{LINK_RESOLVE}"),
            protocol_id: LINK_RESOLVE.into(),
            runner_hint: Some(RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "message".into(),
                title: "发送消息".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(
                    mutsuki_bot_protocol::BOT_FLOW_MESSAGE_SEND_TYPE,
                    1,
                ),
                required: false,
            },
        ],
        config_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "url": {"type": "string", "title": "Bilibili 链接"},
                "outbound_binding": {"type": "string", "title": "出站绑定"},
                "cooldown_ms": {
                    "type": "integer",
                    "title": "冷却（毫秒）",
                    "minimum": 0
                }
            }
        }),
    });
    if management_nodes {
        nodes.push(BotNodeDescriptor {
            node_type_id: "mutsuki.bot.bilibili.management".into(),
            version: 1,
            title: "Bilibili 管理".into(),
            category: "Bilibili".into(),
            role: BotNodeRole::Processor,
            binding: Some(BotNodeBinding {
                binding_id: format!("binding:{MANAGEMENT_COMMAND}"),
                protocol_id: MANAGEMENT_COMMAND.into(),
                runner_hint: Some(RUNNER_ID.into()),
            }),
            ports: vec![
                BotNodePortDescriptor {
                    port_id: "command".into(),
                    title: "管理请求".into(),
                    direction: BotNodePortDirection::Input,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.command.event", 1),
                    required: true,
                },
                BotNodePortDescriptor {
                    port_id: "message".into(),
                    title: "回复消息".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new(
                        mutsuki_bot_protocol::BOT_FLOW_MESSAGE_SEND_TYPE,
                        1,
                    ),
                    required: false,
                },
            ],
            config_schema: json!({"type": "object", "additionalProperties": false}),
        });
    }
    if !nodes.is_empty() {
        builder = builder.extension(
            BotNodeCatalogFragment { nodes }
                .into_plugin_extension()
                .expect("Bilibili node catalog serializes"),
        );
    }
    let mut manifest = builder.build().manifest;
    for protocol_id in manifest.provides.runners[0]
        .accepted_protocol_ids
        .iter()
        .cloned()
    {
        manifest
            .provides
            .protocol_classes
            .insert(protocol_id, ProtocolClass::Effect);
    }
    manifest
}

pub(crate) fn runner_descriptor(risk_control: bool) -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID);
    let protocols: &[&str] = &[
        POLL_LIVE,
        POLL_DYNAMIC,
        POLL_VIDEO,
        NOTIFY_CARD,
        LINK_RESOLVE,
    ];
    for protocol in protocols {
        builder = builder.accepted_protocol(*protocol);
    }
    builder = builder
        .accepted_protocol(MANAGEMENT_COMMAND)
        .requires_protocol(QR_RENDER);
    if risk_control {
        builder = builder.requires_protocol(SNAPSHOT);
    }
    builder
        .requires_protocol(CARD_RENDER)
        .purity(RunnerPurity::Effectful)
        // Polls, link resolution and card media use the blocking secure-fetch
        // transport; declaring Io keeps that I/O off the orchestration domain
        // that drives Flow.
        .execution_class(ExecutionClass::Io)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            max_inflight_batches: 1,
            ..Default::default()
        })
        .metadata("domain", ScalarValue::String("bilibili".into()))
        .metadata("backend", ScalarValue::String("web_cookie".into()))
        .build()
}

fn protocol(id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(id)
        .input_schema(json!({"type":"object"}))
        .output_schema(json!({"type":"object"}))
        .error_schema(json!({"type":"object"}))
        .build()
}

struct ManifestRunner {
    descriptor: RunnerDescriptor,
}
impl Runner for ManifestRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }
    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_error(
            &batch,
            RuntimeError::new("runner.unavailable", PLUGIN_ID, "manifest_only"),
        ))
    }
}
