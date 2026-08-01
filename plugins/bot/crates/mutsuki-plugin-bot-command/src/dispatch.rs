use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_COMMAND_HANDLE_PROTOCOL_ID, BOT_COMMAND_HELP_PROTOCOL_ID, BOT_COMMAND_PARSE_PROTOCOL_ID,
    BotCommandDescriptor, BotCommandEvent, BotCommandHelpRequest, BotEvent,
    BotHandlerExecutionResult, BotHandlerOutcome, bot_command_binding_id,
};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, ERR_RUNTIME_HOST_FAILED, ExecutionClass, InvocationMode,
    OrderingRequirement, PluginArtifact, PluginManifest, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerDescriptor, RunnerMode, RunnerOrderingCapability,
    RunnerPayloadCapability, RunnerPurity, RunnerResourceCapability, RunnerResult,
    RunnerSideEffect, RuntimeError, ScalarValue, Task, TaskPayload, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AbiHostClient, AbiHostClientV2, LoadedPlugin, PluginBuilder, ProtocolDescriptorBuilder,
    map_work_batch_entries,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandParseError, CommandParser, message_text, validate_command_descriptors};

pub const BOT_COMMAND_PLUGIN_ID: &str = "mutsuki.bot.command";
pub const BOT_COMMAND_RUNNER_ID: &str = "mutsuki.bot.command.parse";

#[derive(Clone, Debug, Deserialize, Serialize, mutsuki_bot_config::MutsukiConfig)]
#[config(
    provider_id = "mutsuki.bot.command",
    title = "Bot Command",
    schema_version = 1,
    value_version = 1
)]
#[serde(deny_unknown_fields)]
pub struct BotCommandConfig {
    #[config(
        title = "指令前缀",
        description = "触发命令解析的前缀列表",
        restart = "plugin_reload"
    )]
    pub prefixes: Vec<String>,
    #[config(
        title = "指令定义",
        description = "命令组、别名与类型参数描述",
        restart = "plugin_reload"
    )]
    #[serde(default)]
    pub commands: Vec<BotCommandDescriptor>,
}

impl BotCommandConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.prefixes.is_empty() || self.prefixes.iter().any(|prefix| prefix.is_empty()) {
            return Err("prefixes must contain non-empty values".into());
        }
        validate_command_descriptors(&self.commands)
    }
}

pub fn bot_command_manifest(plugin_generation: u64) -> PluginManifest {
    PluginBuilder::new(BOT_COMMAND_PLUGIN_ID)
        .runner_descriptor(command_descriptor(plugin_generation))
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_COMMAND_PARSE_PROTOCOL_ID).build(),
            BOT_COMMAND_RUNNER_ID,
            "bot-command-parse",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_COMMAND_HELP_PROTOCOL_ID).build(),
            BOT_COMMAND_RUNNER_ID,
            "bot-command-help",
        )
        .build()
        .manifest
}

pub fn bot_command_abi_manifest(path: &str, sha256: &str) -> PluginManifest {
    command_plugin(
        1,
        vec!["/".into()],
        Vec::new(),
        PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: path.into(),
            sha256: sha256.into(),
            companion_artifacts: Vec::new(),
        },
    )
    .manifest
}

fn command_plugin(
    plugin_generation: u64,
    prefixes: Vec<String>,
    commands: Vec<BotCommandDescriptor>,
    artifact: PluginArtifact,
) -> LoadedPlugin {
    PluginBuilder::new(BOT_COMMAND_PLUGIN_ID)
        .runner(Box::new(BotCommandRunner::with_commands(
            plugin_generation,
            prefixes,
            commands,
        )))
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_COMMAND_PARSE_PROTOCOL_ID).build(),
            BOT_COMMAND_RUNNER_ID,
            "bot-command-parse",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_COMMAND_HELP_PROTOCOL_ID).build(),
            BOT_COMMAND_RUNNER_ID,
            "bot-command-help",
        )
        .artifact(artifact)
        .build()
}

fn create_abi_plugin_v1(_host: AbiHostClient, config: Value) -> RuntimeResult<LoadedPlugin> {
    create_configured_abi_plugin(config)
}

fn create_abi_plugin_v2(_host: AbiHostClientV2, config: Value) -> RuntimeResult<LoadedPlugin> {
    create_configured_abi_plugin(config)
}

fn create_configured_abi_plugin(config: Value) -> RuntimeResult<LoadedPlugin> {
    let config: BotCommandConfig = serde_json::from_value(config)
        .map_err(|error| RuntimeFailure::new(failure("mutsuki.bot.command.config", error)))?;
    config
        .validate()
        .map_err(|error| RuntimeFailure::new(failure("mutsuki.bot.command.config", error)))?;
    Ok(command_plugin(
        1,
        config.prefixes,
        config.commands,
        PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: "plugin".into(),
            sha256: "sha256:plugin".into(),
            companion_artifacts: Vec::new(),
        },
    ))
}

pub struct BotCommandRunner {
    descriptor: RunnerDescriptor,
    parser: CommandParser,
}

impl BotCommandRunner {
    pub fn new(plugin_generation: u64, prefixes: Vec<String>) -> Self {
        Self::with_commands(plugin_generation, prefixes, Vec::new())
    }

    pub fn with_commands(
        plugin_generation: u64,
        prefixes: Vec<String>,
        commands: Vec<BotCommandDescriptor>,
    ) -> Self {
        Self {
            descriptor: command_descriptor(plugin_generation),
            parser: CommandParser::new(prefixes).commands(commands),
        }
    }
}

impl Runner for BotCommandRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            if task.protocol_id == BOT_COMMAND_HELP_PROTOCOL_ID {
                let request: BotCommandHelpRequest =
                    serde_json::from_value(task.payload.to_value())
                        .map_err(|error| failure("mutsuki.bot.command.help.decode", error))?;
                let mut result = RunnerResult::completed(task.task_id.clone());
                result.output = Some(
                    serde_json::to_value(self.parser.help(&request.path))
                        .map_err(|error| failure("mutsuki.bot.command.help.encode", error))?,
                );
                return Ok(result);
            }
            let event = task
                .payload
                .decode_shared::<BotEvent>()
                .map_err(|error| failure("mutsuki.bot.command.decode", error))?;
            let Some(text) = message_text(event.as_ref()) else {
                return Ok(handler_result(&task.task_id, BotHandlerOutcome::Continue));
            };
            let command = match self.parser.parse(&text) {
                Ok(command) => command,
                Err(CommandParseError::MissingPrefix) => {
                    return Ok(handler_result(&task.task_id, BotHandlerOutcome::Continue));
                }
                Err(error) => {
                    let attempted_path = text
                        .trim_start_matches(|ch: char| !ch.is_alphanumeric())
                        .split_whitespace()
                        .take(2)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    return Err(parse_failure(
                        "mutsuki.bot.command.parse",
                        self.parser.parse_failure(&error, &attempted_path),
                    ));
                }
            };
            let command_event = BotCommandEvent {
                source: (*event).clone(),
                name: command.name,
                args: command.args,
                command_path: command.command_path,
                typed_args: command.typed_args,
                raw_text: command.raw_text,
            };
            tracing::info!(
                account_id = %command_event.source.bot.account_id,
                event_id = %command_event.source.event_id,
                task_id = %task.task_id,
                runner_id = BOT_COMMAND_RUNNER_ID,
                command = %command_event.name,
                correlation_id = task.correlation_id.as_deref().unwrap_or(""),
                "Bot command parsed"
            );
            let binding_id = bot_command_binding_id(&command_event.name);
            let mut child = Task::new(
                format!("mutsuki.bot.command.handle:{}", task.task_id),
                BOT_COMMAND_HANDLE_PROTOCOL_ID,
                TaskPayload::from_local(command_event),
            );
            child.registry_generation = ctx.registry_generation;
            child.trace_id = task.trace_id.clone();
            child.correlation_id = task.correlation_id.clone();
            child.target_binding_id = Some(binding_id);
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(
                serde_json::to_value(BotHandlerExecutionResult {
                    outcome: BotHandlerOutcome::Stop,
                })
                .map_err(|error| failure("mutsuki.bot.command.outcome", error))?,
            );
            result.tasks.push(child);
            Ok(result)
        })
    }
}

fn handler_result(task_id: &str, outcome: BotHandlerOutcome) -> RunnerResult {
    let mut result = RunnerResult::completed(task_id);
    result.output = Some(
        serde_json::to_value(BotHandlerExecutionResult { outcome })
            .expect("handler outcome serializes"),
    );
    result
}

pub fn command_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: BOT_COMMAND_RUNNER_ID.into(),
        plugin_id: BOT_COMMAND_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![
            BOT_COMMAND_PARSE_PROTOCOL_ID.into(),
            BOT_COMMAND_HELP_PROTOCOL_ID.into(),
        ],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Orchestration,
        invocation_mode: InvocationMode::SyncExclusive,
        concurrency: RunnerConcurrency::Exclusive,
        input_schema: json!({
            "type": "object",
            "required": ["event_id", "message"]
        }),
        output_schema: json!({
            "tasks": [BOT_COMMAND_HANDLE_PROTOCOL_ID]
        }),
        batch: RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 32,
            max_batch_entries: 128,
            max_entry_concurrency: 32,
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
            ScalarValue::String("Bot command parser".into()),
        )]),
        contract_surfaces: vec![
            format!("runner:{BOT_COMMAND_RUNNER_ID}"),
            format!("task_protocol:{BOT_COMMAND_PARSE_PROTOCOL_ID}"),
        ],
    }
}

fn failure(route: impl Into<String>, error: impl std::fmt::Display) -> RuntimeError {
    let mut runtime_error =
        RuntimeError::new(ERR_RUNTIME_HOST_FAILED, BOT_COMMAND_PLUGIN_ID, route.into());
    runtime_error
        .evidence
        .insert("message".into(), ScalarValue::String(error.to_string()));
    runtime_error
}

fn parse_failure(
    route: impl Into<String>,
    failure: mutsuki_bot_protocol::BotCommandParseFailure,
) -> RuntimeError {
    let code = match failure.code {
        mutsuki_bot_protocol::BotCommandParseErrorCode::MissingPrefix => "missing_prefix",
        mutsuki_bot_protocol::BotCommandParseErrorCode::EmptyName => "empty_name",
        mutsuki_bot_protocol::BotCommandParseErrorCode::UnterminatedQuote => "unterminated_quote",
        mutsuki_bot_protocol::BotCommandParseErrorCode::UnknownCommand => "unknown_command",
        mutsuki_bot_protocol::BotCommandParseErrorCode::MissingArgument => "missing_argument",
        mutsuki_bot_protocol::BotCommandParseErrorCode::InvalidArgument => "invalid_argument",
        mutsuki_bot_protocol::BotCommandParseErrorCode::UnexpectedArgument => "unexpected_argument",
    };
    let mut error = RuntimeError::new(
        format!("bot.command.parse.{code}"),
        BOT_COMMAND_PLUGIN_ID,
        route.into(),
    );
    error.evidence.insert(
        "failure".into(),
        ScalarValue::String(
            serde_json::to_string(&failure).unwrap_or_else(|_| failure.message.clone()),
        ),
    );
    error
}

mutsuki_runtime_sdk::export_mutsuki_plugin_abi_v1!(create_abi_plugin_v1);
mutsuki_runtime_sdk::export_mutsuki_plugin_abi_v2!(create_abi_plugin_v2);

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{BotAccountRef, BotEventKind, BotMessage, BotPlatform, BotTarget};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, WorkResourcePlan,
    };

    #[test]
    fn command_runner_batches_parse_tasks_and_isolates_decode_failure() {
        let mut runner = BotCommandRunner::new(1, vec!["/".into()]);
        let valid_a = command_task("task-a", "event-a", "/echo one");
        let invalid = Task::new("task-invalid", BOT_COMMAND_PARSE_PROTOCOL_ID, json!({}));
        let valid_b = command_task("task-b", "event-b", "/ping two");

        let completion = runner
            .run_batch(test_context(11, 3), batch(vec![valid_a, invalid, valid_b]))
            .unwrap();

        assert_eq!(completion.results.len(), 3);
        assert!(completion.results[1].result.is_none());
        assert!(completion.results[1].error.is_some());
        let first = &completion.results[0].result.as_ref().unwrap().tasks[0];
        let third = &completion.results[2].result.as_ref().unwrap().tasks[0];
        assert_eq!(first.registry_generation, 11);
        assert_eq!(third.registry_generation, 11);
        let first_command: BotCommandEvent = first.payload.decode().unwrap();
        let third_command: BotCommandEvent = third.payload.decode().unwrap();
        assert_eq!(first_command.name, "echo");
        assert_eq!(first_command.args, ["one"]);
        assert_eq!(third_command.name, "ping");
        assert_eq!(third_command.args, ["two"]);
        assert_eq!(
            first.target_binding_id.as_deref(),
            Some("binding:mutsuki.bot.command/echo@1")
        );
        assert_eq!(
            third.target_binding_id.as_deref(),
            Some("binding:mutsuki.bot.command/ping@1")
        );
    }

    #[test]
    fn builtin_and_abi_artifacts_expose_the_same_business_surface() {
        let builtin = bot_command_manifest(1);
        let abi = bot_command_abi_manifest("command.dll", &format!("sha256:{}", "1".repeat(64)));
        assert_eq!(builtin.business_surface(), abi.business_surface());
    }

    fn command_task(task_id: &str, event_id: &str, text: &str) -> Task {
        let target = BotTarget::User {
            user_id: "user".into(),
        };
        Task::new(
            task_id,
            BOT_COMMAND_PARSE_PROTOCOL_ID,
            TaskPayload::from_local(BotEvent {
                event_id: event_id.into(),
                platform: BotPlatform::QqBot,
                bot: BotAccountRef {
                    account_id: "main".into(),
                    platform: BotPlatform::QqBot,
                },
                kind: BotEventKind::MessageCreated,
                time_ms: 1,
                target: target.clone(),
                actor: None,
                message: Some(BotMessage::text(target, text)),
                raw: None,
                ext: Default::default(),
            }),
        )
    }

    fn batch(tasks: Vec<Task>) -> WorkBatch {
        WorkBatch {
            batch_id: "batch:command".into(),
            tick_id: "tick:command".into(),
            batch_key: BOT_COMMAND_RUNNER_ID.into(),
            entries: entries(&tasks),
            payload: BatchPayload::from_tasks(&tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn entries(tasks: &[Task]) -> Vec<BatchEntry> {
        tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: format!("entry-{index}"),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: None,
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::PreserveSubmitOrder,
            })
            .collect()
    }

    fn test_context(registry_generation: u64, entry_count: usize) -> RunnerContext {
        RunnerContext::new(
            registry_generation,
            1,
            "executor:command",
            Vec::<String>::new(),
            "batch:command",
        )
        .with_batch("batch:command", entry_count)
    }
}
