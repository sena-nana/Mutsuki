use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotCommandEvent, BotFlowEventEnvelope, BotFlowPayload,
    BotFlowTypeRef, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation,
    BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole,
};
use mutsuki_bot_sdk::MessageBuilder;
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, InvocationMode, OrderingRequirement, RunnerBatchCapability,
    RunnerConcurrency, RunnerControlCapability, RunnerDescriptor, RunnerMode,
    RunnerOrderingCapability, RunnerPayloadCapability, RunnerPurity, RunnerResourceCapability,
    RunnerSideEffect, RuntimeError, ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{PluginBuilder, ProtocolDescriptorBuilder, map_work_batch_entries};
use serde_json::json;

pub const ECHO_PLUGIN_ID: &str = "example.bot.echo";
pub const ECHO_RUNNER_ID: &str = "example.bot.echo.command";
pub const ECHO_PROTOCOL_ID: &str = "example.bot.echo/reply@1";
pub const PING_PROTOCOL_ID: &str = "example.bot.ping/reply@1";

pub fn echo_manifest(plugin_generation: u64) -> mutsuki_runtime_contracts::PluginManifest {
    PluginBuilder::new(ECHO_PLUGIN_ID)
        .runner_descriptor(echo_descriptor(plugin_generation))
        .protocol_handler(
            reply_protocol_descriptor(ECHO_PROTOCOL_ID),
            ECHO_RUNNER_ID,
            "echo-reply",
        )
        .protocol_handler(
            reply_protocol_descriptor(PING_PROTOCOL_ID),
            ECHO_RUNNER_ID,
            "ping-reply",
        )
        .extension(
            BotNodeCatalogFragment {
                nodes: vec![
                    reply_node("example.bot.echo", "回显", ECHO_PROTOCOL_ID),
                    reply_node("example.bot.ping", "Ping", PING_PROTOCOL_ID),
                ],
            }
            .into_plugin_extension()
            .expect("echo node catalog serializes"),
        )
        .build()
        .manifest
}

fn reply_protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({
            "type": "object",
            "required": ["flow_id", "node_id", "input"]
        }))
        .output_schema(json!({
            "type": "object",
            "required": ["outputs", "metadata"]
        }))
        .error_schema(json!({
            "type": "object",
            "required": ["code", "source", "route"]
        }))
        .build()
}

fn reply_node(node_type_id: &str, title: &str, protocol_id: &str) -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: node_type_id.into(),
        version: 1,
        title: title.into(),
        category: "示例".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{protocol_id}"),
            protocol_id: protocol_id.into(),
            runner_hint: Some(ECHO_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "command".into(),
                title: "命令".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new("mutsuki.bot.command.event", 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "message".into(),
                title: "消息".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                required: false,
            },
        ],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    }
}

pub fn echo_runner(plugin_generation: u64) -> Box<dyn Runner> {
    Box::new(EchoCommandRunner::new(plugin_generation))
}

struct EchoCommandRunner {
    descriptor: RunnerDescriptor,
}

impl EchoCommandRunner {
    fn new(plugin_generation: u64) -> Self {
        Self {
            descriptor: echo_descriptor(plugin_generation),
        }
    }
}

impl Runner for EchoCommandRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let invocation = task
                .payload
                .decode::<BotNodeInvocation>()
                .map_err(|error| echo_error(format!("echo.command.decode:{error}")))?;
            let command: BotCommandEvent =
                serde_json::from_value(invocation.input.payload.value.clone())
                    .map_err(|error| echo_error(format!("echo.command.decode:{error}")))?;
            let mut result =
                mutsuki_runtime_contracts::RunnerResult::completed(task.task_id.clone());
            let message = reply_message(&command, task.protocol_id.as_str());
            result.output = Some(
                serde_json::to_value(BotNodeResult {
                    outputs: vec![BotNodeOutput {
                        port_id: "message".into(),
                        event: BotFlowEventEnvelope {
                            event_id: format!("{}:reply", invocation.input.event_id),
                            protocol_id: BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
                            payload: BotFlowPayload {
                                event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                                value: serde_json::to_value(message)
                                    .expect("BotMessage serializes"),
                            },
                            context: invocation.input.context,
                            trace_id: invocation.input.trace_id,
                            correlation_id: invocation.input.correlation_id,
                        },
                    }],
                    metadata: Default::default(),
                })
                .expect("BotNodeResult serializes"),
            );
            Ok(result)
        })
    }
}

fn reply_message(command: &BotCommandEvent, protocol_id: &str) -> mutsuki_bot_protocol::BotMessage {
    let text = match protocol_id {
        PING_PROTOCOL_ID => "pong".to_string(),
        ECHO_PROTOCOL_ID => command.args.join(" "),
        _ => String::new(),
    };
    let mut message = MessageBuilder::new(command.source.target.clone()).text(text);
    if let Some(message_id) = command
        .source
        .message
        .as_ref()
        .and_then(|message| message.message_id.clone())
    {
        message = message.reply_to(message_id);
    }
    message.build()
}

fn echo_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: ECHO_RUNNER_ID.into(),
        plugin_id: ECHO_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![ECHO_PROTOCOL_ID.into(), PING_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Orchestration,
        invocation_mode: InvocationMode::SyncExclusive,
        concurrency: RunnerConcurrency::Exclusive,
        input_schema: json!({
            "type": "object",
            "required": ["source", "name", "args"]
        }),
        output_schema: json!({
            "tasks": [BOT_MESSAGE_SEND_PROTOCOL_ID]
        }),
        batch: RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 16,
            max_batch_entries: 64,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        },
        payload: RunnerPayloadCapability::default(),
        resources: RunnerResourceCapability {
            requires_resource_plan: false,
            ..Default::default()
        },
        ordering: RunnerOrderingCapability {
            default: OrderingRequirement::PreserveSubmitOrder,
            supports_sequence: true,
            supports_same_resource_order: true,
        },
        control: RunnerControlCapability::default(),
        metadata: BTreeMap::from([(
            "description".into(),
            ScalarValue::String("Platform-neutral example echo command handler".into()),
        )]),
        contract_surfaces: vec![
            format!("runner:{ECHO_RUNNER_ID}"),
            format!("task_protocol:{ECHO_PROTOCOL_ID}"),
            format!("task_protocol:{PING_PROTOCOL_ID}"),
        ],
    }
}

fn echo_error(route: impl Into<String>) -> RuntimeError {
    RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        ECHO_PLUGIN_ID,
        route,
    )
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BotAccountRef, BotEvent, BotEventKind, BotExtMap, BotMessage, BotPlatform, BotTarget,
    };

    use super::*;

    #[test]
    fn echo_and_ping_build_standard_reply_messages_without_platform_dependencies() {
        let mut command = command("echo", vec!["hello".into(), "world".into()]);
        let echo = reply_message(&command, ECHO_PROTOCOL_ID);
        assert_eq!(echo.plain_text(), "hello world");
        assert_eq!(echo.reply_to.as_deref(), Some("source-message"));

        command.name = "ping".into();
        let ping = reply_message(&command, PING_PROTOCOL_ID);
        assert_eq!(ping.plain_text(), "pong");
        assert_eq!(
            ping.target,
            BotTarget::User {
                user_id: "user".into()
            }
        );
    }

    fn command(name: &str, args: Vec<String>) -> BotCommandEvent {
        let target = BotTarget::User {
            user_id: "user".into(),
        };
        BotCommandEvent {
            source: BotEvent {
                event_id: "event".into(),
                platform: BotPlatform::Custom("test-platform".into()),
                bot: BotAccountRef {
                    account_id: "bot".into(),
                    platform: BotPlatform::Custom("test-platform".into()),
                },
                kind: BotEventKind::MessageCreated,
                time_ms: 1,
                target: target.clone(),
                actor: None,
                message: Some(BotMessage {
                    message_id: Some("source-message".into()),
                    target,
                    sender: None,
                    segments: Vec::new(),
                    reply_to: None,
                    time_ms: None,
                    ext: BotExtMap::new(),
                }),
                raw: None,
                ext: BotExtMap::new(),
            },
            name: name.into(),
            args,
            command_path: vec![name.into()],
            typed_args: Default::default(),
            raw_text: format!("/{name}"),
        }
    }
}
