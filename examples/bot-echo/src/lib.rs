use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_COMMAND_HANDLE_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID, BotCommandEvent,
    bot_command_binding_id,
};
use mutsuki_bot_sdk::MessageBuilder;
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, InvocationMode, OrderingRequirement, RunnerBatchCapability,
    RunnerConcurrency, RunnerControlCapability, RunnerDescriptor, RunnerMode,
    RunnerOrderingCapability, RunnerPayloadCapability, RunnerPurity, RunnerResourceCapability,
    RunnerSideEffect, RuntimeError, ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{HandlerBindingBuilder, PluginBuilder, map_work_batch_entries};
use serde_json::json;

pub const ECHO_PLUGIN_ID: &str = "example.bot.echo";
pub const ECHO_RUNNER_ID: &str = "example.bot.echo.command";

pub fn echo_manifest(plugin_generation: u64) -> mutsuki_runtime_contracts::PluginManifest {
    let mut builder =
        PluginBuilder::new(ECHO_PLUGIN_ID).runner_descriptor(echo_descriptor(plugin_generation));
    for command in ["echo", "ping"] {
        builder = builder.handler_binding(
            HandlerBindingBuilder::new(
                bot_command_binding_id(command),
                ECHO_PLUGIN_ID,
                BOT_COMMAND_HANDLE_PROTOCOL_ID,
                BOT_COMMAND_HANDLE_PROTOCOL_ID,
            )
            .target_runner_hint(ECHO_RUNNER_ID)
            .pool_id("orchestration")
            .build(),
        );
    }
    builder.build().manifest
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
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let command: BotCommandEvent = serde_json::from_value(task.payload.clone())
                .map_err(|error| echo_error(format!("echo.command.decode:{error}")))?;
            let mut result =
                mutsuki_runtime_contracts::RunnerResult::completed(task.task_id.clone());
            if let Some(message) = reply_message(&command) {
                let mut send = Task::new(
                    format!("example.bot.echo.send:{}", command.source.event_id),
                    BOT_MESSAGE_SEND_PROTOCOL_ID,
                    serde_json::to_value(message)
                        .map_err(|error| echo_error(format!("echo.message.encode:{error}")))?,
                );
                send.trace_id = task.trace_id.clone();
                send.correlation_id = task.correlation_id.clone();
                tracing::info!(
                    account_id = %command.source.bot.account_id,
                    event_id = %command.source.event_id,
                    task_id = %task.task_id,
                    runner_id = ECHO_RUNNER_ID,
                    command = %command.name,
                    reply_request_id = %send.task_id,
                    correlation_id = task.correlation_id.as_deref().unwrap_or(""),
                    "Example Bot reply requested"
                );
                send.registry_generation = ctx.registry_generation;
                result.tasks.push(send);
            }
            Ok(result)
        })
    }
}

fn reply_message(command: &BotCommandEvent) -> Option<mutsuki_bot_protocol::BotMessage> {
    let text = match command.name.as_str() {
        "ping" => "pong".to_string(),
        "echo" => command.args.join(" "),
        _ => return None,
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
    Some(message.build())
}

fn echo_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: ECHO_RUNNER_ID.into(),
        plugin_id: ECHO_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![BOT_COMMAND_HANDLE_PROTOCOL_ID.into()],
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
            format!("task_protocol:{BOT_COMMAND_HANDLE_PROTOCOL_ID}"),
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
        let echo = reply_message(&command).expect("echo reply");
        assert_eq!(echo.plain_text(), "hello world");
        assert_eq!(echo.reply_to.as_deref(), Some("source-message"));

        command.name = "ping".into();
        let ping = reply_message(&command).expect("ping reply");
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
            raw_text: format!("/{name}"),
        }
    }
}
