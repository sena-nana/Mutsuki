use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_COMMAND_HANDLE_PROTOCOL_ID, BOT_COMMAND_PARSE_PROTOCOL_ID, BotCommandDescriptor,
    BotCommandEvent, BotEvent, BotFlowEventEnvelope, BotFlowPayload, BotFlowTypeRef,
    BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodeOutput,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole,
};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, ERR_RUNTIME_HOST_FAILED, ExecutionClass, InvocationMode,
    OrderingRequirement, PluginArtifact, PluginManifest, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerDescriptor, RunnerMode, RunnerOrderingCapability,
    RunnerPayloadCapability, RunnerPurity, RunnerResourceCapability, RunnerResult,
    RunnerSideEffect, RuntimeError, ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AbiHostClient, LoadedPlugin, PluginBuilder, ProtocolDescriptorBuilder, map_work_batch_entries,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandParseError, CommandParser, message_text, validate_command_descriptors};

pub const BOT_COMMAND_PLUGIN_ID: &str = "mutsuki.bot.command";
pub const BOT_COMMAND_RUNNER_ID: &str = "mutsuki.bot.command.parse";
pub const BOT_COMMAND_MATCH_NODE_TYPE_ID: &str = "mutsuki.bot.command.match";
pub const BOT_COMMAND_EVENT_TYPE_ID: &str = "mutsuki.bot.command.event";
pub const BOT_EVENT_TYPE_ID: &str = "mutsuki.bot.event";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BotCommandPluginConfig {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BotCommandMatchConfig {
    pub prefixes: Vec<String>,
    pub path: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<Vec<String>>,
    #[serde(default)]
    pub arguments: Vec<mutsuki_bot_protocol::BotCommandArgumentDescriptor>,
    #[serde(default)]
    pub case_sensitive: bool,
}

impl BotCommandMatchConfig {
    fn parser(&self) -> Result<CommandParser, String> {
        if self.prefixes.is_empty() || self.prefixes.iter().any(|prefix| prefix.is_empty()) {
            return Err("prefixes must contain non-empty values".into());
        }
        let descriptor = BotCommandDescriptor {
            path: self.path.clone(),
            aliases: self.aliases.clone(),
            arguments: self.arguments.clone(),
            summary: None,
        };
        validate_command_descriptors(std::slice::from_ref(&descriptor))?;
        Ok(CommandParser::new(self.prefixes.clone())
            .commands(vec![descriptor])
            .case_sensitive(self.case_sensitive))
    }
}

pub fn bot_command_manifest(plugin_generation: u64) -> PluginManifest {
    PluginBuilder::new(BOT_COMMAND_PLUGIN_ID)
        .runner_descriptor(command_node_descriptor(plugin_generation))
        .protocol_handler(
            command_protocol_descriptor(),
            BOT_COMMAND_RUNNER_ID,
            "bot-command-parse",
        )
        .extension(
            command_node_catalog()
                .into_plugin_extension()
                .expect("command node catalog serializes"),
        )
        .build()
        .manifest
}

fn command_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![BotNodeDescriptor {
            node_type_id: BOT_COMMAND_MATCH_NODE_TYPE_ID.into(),
            version: 1,
            title: "命令匹配".into(),
            category: "匹配".into(),
            role: BotNodeRole::Match,
            binding: Some(BotNodeBinding {
                binding_id: format!("binding:{BOT_COMMAND_PARSE_PROTOCOL_ID}"),
                protocol_id: BOT_COMMAND_PARSE_PROTOCOL_ID.into(),
                runner_hint: Some(BOT_COMMAND_RUNNER_ID.into()),
            }),
            ports: vec![
                BotNodePortDescriptor {
                    port_id: "event".into(),
                    title: "消息事件".into(),
                    direction: BotNodePortDirection::Input,
                    event_type: BotFlowTypeRef::new(BOT_EVENT_TYPE_ID, 1),
                    required: true,
                },
                BotNodePortDescriptor {
                    port_id: "matched".into(),
                    title: "已匹配".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new(BOT_COMMAND_EVENT_TYPE_ID, 1),
                    required: false,
                },
                BotNodePortDescriptor {
                    port_id: "unmatched".into(),
                    title: "未匹配".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new(BOT_EVENT_TYPE_ID, 1),
                    required: false,
                },
            ],
            config_schema: json!({
                "type": "object",
                "required": ["prefixes", "path"],
                "additionalProperties": false,
                "properties": {
                    "prefixes": {"type": "array", "minItems": 1, "items": {"type": "string", "minLength": 1}, "title": "命令前缀"},
                    "path": {"type": "array", "minItems": 1, "items": {"type": "string", "minLength": 1}, "title": "命令路径"},
                    "aliases": {"type": "array", "items": {"type": "array", "items": {"type": "string"}}, "title": "别名"},
                    "arguments": {
                        "type": "array",
                        "title": "类型化参数",
                        "items": {
                            "type": "object",
                            "required": ["name", "kind"],
                            "additionalProperties": false,
                            "properties": {
                                "name": {"type": "string", "minLength": 1},
                                "kind": {"type": "string", "enum": ["string", "integer", "number", "boolean"]},
                                "optional": {"type": "boolean", "default": false},
                                "variadic": {"type": "boolean", "default": false},
                                "default": {}
                            }
                        }
                    },
                    "case_sensitive": {"type": "boolean", "default": false, "title": "区分大小写"}
                }
            }),
        }],
    }
}

pub fn bot_command_abi_manifest(path: &str, sha256: &str) -> PluginManifest {
    command_plugin(
        1,
        PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: path.into(),
            sha256: sha256.into(),
            companion_artifacts: Vec::new(),
        },
    )
    .manifest
}

fn command_plugin(plugin_generation: u64, artifact: PluginArtifact) -> LoadedPlugin {
    PluginBuilder::new(BOT_COMMAND_PLUGIN_ID)
        .runner(Box::new(BotCommandNodeRunner::new(plugin_generation)))
        .protocol_handler(
            command_protocol_descriptor(),
            BOT_COMMAND_RUNNER_ID,
            "bot-command-parse",
        )
        .extension(
            command_node_catalog()
                .into_plugin_extension()
                .expect("command node catalog serializes"),
        )
        .artifact(artifact)
        .build()
}

fn command_protocol_descriptor() -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(BOT_COMMAND_PARSE_PROTOCOL_ID)
        .input_schema(json!({
            "type": "object",
            "oneOf": [
                {"required": ["flow_id", "node_id", "input"]},
                {"required": ["event_id", "message"]}
            ]
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

pub struct BotCommandNodeRunner {
    descriptor: RunnerDescriptor,
}

impl BotCommandNodeRunner {
    pub fn new(plugin_generation: u64) -> Self {
        Self {
            descriptor: command_node_descriptor(plugin_generation),
        }
    }
}

impl Runner for BotCommandNodeRunner {
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
                .decode_shared::<BotNodeInvocation>()
                .map_err(|error| failure("mutsuki.bot.command.node.decode", error))?;
            let config: BotCommandMatchConfig =
                serde_json::from_value(invocation.config.clone())
                    .map_err(|error| failure("mutsuki.bot.command.node.config", error))?;
            let parser = config
                .parser()
                .map_err(|error| failure("mutsuki.bot.command.node.config", error))?;
            let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
                .map_err(|error| failure("mutsuki.bot.command.node.event", error))?;
            let output = match message_text(&event).map(|text| (text.clone(), parser.parse(&text)))
            {
                None
                | Some((
                    _,
                    Err(CommandParseError::MissingPrefix | CommandParseError::UnknownCommand(_)),
                )) => BotNodeOutput {
                    port_id: "unmatched".into(),
                    event: invocation.input.clone(),
                },
                Some((_, Err(error))) => {
                    return Err(parse_failure(
                        "mutsuki.bot.command.node.parse",
                        parser.parse_failure(&error, &[]),
                    ));
                }
                Some((_, Ok(command))) => {
                    let command_event = BotCommandEvent {
                        source: event,
                        name: command.name,
                        args: command.args,
                        command_path: command.command_path,
                        typed_args: command.typed_args,
                        raw_text: command.raw_text,
                    };
                    BotNodeOutput {
                        port_id: "matched".into(),
                        event: BotFlowEventEnvelope {
                            event_id: invocation.input.event_id.clone(),
                            protocol_id: BOT_COMMAND_HANDLE_PROTOCOL_ID.into(),
                            payload: BotFlowPayload {
                                event_type: BotFlowTypeRef::new(BOT_COMMAND_EVENT_TYPE_ID, 1),
                                value: serde_json::to_value(command_event).map_err(|error| {
                                    failure("mutsuki.bot.command.node.output", error)
                                })?,
                            },
                            context: invocation.input.context.clone(),
                            trace_id: invocation.input.trace_id.clone(),
                            correlation_id: invocation.input.correlation_id.clone(),
                        },
                    }
                }
            };
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(
                serde_json::to_value(BotNodeResult {
                    outputs: vec![output],
                    metadata: BTreeMap::new(),
                })
                .map_err(|error| failure("mutsuki.bot.command.node.result", error))?,
            );
            Ok(result)
        })
    }
}

fn command_node_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: BOT_COMMAND_RUNNER_ID.into(),
        plugin_id: BOT_COMMAND_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![BOT_COMMAND_PARSE_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Orchestration,
        invocation_mode: InvocationMode::SyncExclusive,
        concurrency: RunnerConcurrency::Exclusive,
        input_schema: json!({"type": "object", "required": ["node_id", "config", "input"]}),
        output_schema: json!({"type": "object", "required": ["outputs"]}),
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
            ScalarValue::String("Bot Flow command match node".into()),
        )]),
        contract_surfaces: vec![
            format!("runner:{BOT_COMMAND_RUNNER_ID}"),
            format!("task_protocol:{BOT_COMMAND_PARSE_PROTOCOL_ID}"),
        ],
    }
}

fn create_abi_plugin(_host: AbiHostClient, config: Value) -> RuntimeResult<LoadedPlugin> {
    create_configured_abi_plugin(config)
}

fn create_configured_abi_plugin(config: Value) -> RuntimeResult<LoadedPlugin> {
    let config = if config.is_null() {
        Value::Object(Default::default())
    } else {
        config
    };
    let _config: BotCommandPluginConfig = serde_json::from_value(config)
        .map_err(|error| RuntimeFailure::new(failure("mutsuki.bot.command.config", error)))?;
    Ok(command_plugin(
        1,
        PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: "plugin".into(),
            sha256: "sha256:plugin".into(),
            companion_artifacts: Vec::new(),
        },
    ))
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

mutsuki_runtime_sdk::export_mutsuki_plugin_abi_v2!(create_abi_plugin);

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        BotAccountRef, BotCommandArgumentDescriptor, BotCommandArgumentKind,
        BotCommandArgumentValue, BotEventKind, BotFlowContext, BotMessage, BotPlatform, BotTarget,
    };
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, Task, TaskPayload, WorkResourcePlan,
    };

    #[test]
    fn command_node_emits_explicit_matched_and_unmatched_ports() {
        let matched = run_node(
            "/echo one",
            json!({
                "prefixes": ["/"],
                "path": ["echo"],
                "aliases": [],
                "arguments": [
                    BotCommandArgumentDescriptor {
                        name: "values".into(),
                        kind: BotCommandArgumentKind::String,
                        optional: true,
                        variadic: true,
                        default: None,
                    }
                ]
            }),
        );
        assert_eq!(matched.outputs[0].port_id, "matched");
        let command: BotCommandEvent =
            serde_json::from_value(matched.outputs[0].event.payload.value.clone()).unwrap();
        assert_eq!(command.command_path, ["echo"]);
        assert_eq!(command.args, ["one"]);

        let unmatched = run_node(
            "hello",
            json!({
                "prefixes": ["/"], "path": ["echo"], "aliases": [], "arguments": []
            }),
        );
        assert_eq!(unmatched.outputs[0].port_id, "unmatched");
    }

    #[test]
    fn command_node_uses_graph_owned_alias_and_typed_arguments() {
        let result = run_node(
            "/a b Alice 7",
            json!({
                "prefixes": ["/"],
                "path": ["admin", "ban"],
                "aliases": [["a", "b"]],
                "arguments": [
                    BotCommandArgumentDescriptor { name: "user".into(), kind: BotCommandArgumentKind::String, optional: false, variadic: false, default: None },
                    BotCommandArgumentDescriptor { name: "days".into(), kind: BotCommandArgumentKind::Integer, optional: false, variadic: false, default: None }
                ]
            }),
        );
        let command: BotCommandEvent =
            serde_json::from_value(result.outputs[0].event.payload.value.clone()).unwrap();

        assert_eq!(command.name, "admin.ban");
        assert_eq!(command.command_path, ["admin", "ban"]);
        assert_eq!(command.args, ["Alice", "7"]);
        assert_eq!(
            command.typed_args["days"],
            BotCommandArgumentValue::Integer(7)
        );
    }

    #[test]
    fn builtin_and_abi_artifacts_expose_the_same_business_surface() {
        let builtin = bot_command_manifest(1);
        let abi = bot_command_abi_manifest("command.dll", &format!("sha256:{}", "1".repeat(64)));
        assert_eq!(builtin.business_surface(), abi.business_surface());
    }

    fn run_node(text: &str, config: Value) -> BotNodeResult {
        let task = command_task("task", "event", text, config);
        let mut runner = BotCommandNodeRunner::new(1);
        let completion = runner
            .run_batch(test_context(11, 1), batch(vec![task]))
            .unwrap();
        let output = completion.results[0]
            .result
            .as_ref()
            .unwrap()
            .output
            .clone()
            .unwrap();
        serde_json::from_value(output).unwrap()
    }

    fn command_task(task_id: &str, event_id: &str, text: &str, config: Value) -> Task {
        let target = BotTarget::User {
            user_id: "user".into(),
        };
        let event = BotEvent {
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
        };
        Task::new(
            task_id,
            BOT_COMMAND_PARSE_PROTOCOL_ID,
            TaskPayload::from_local(BotNodeInvocation {
                flow_id: "flow".into(),
                graph_revision: 1,
                execution_id: "execution".into(),
                node_id: "command".into(),
                input_port_id: "event".into(),
                config,
                input: BotFlowEventEnvelope {
                    event_id: event_id.into(),
                    protocol_id: "mutsuki.bot.event/ingest@1".into(),
                    payload: BotFlowPayload {
                        event_type: BotFlowTypeRef::new(BOT_EVENT_TYPE_ID, 1),
                        value: serde_json::to_value(event).unwrap(),
                    },
                    context: BotFlowContext {
                        bot: None,
                        target: None,
                        actor: None,
                        ext: Default::default(),
                    },
                    trace_id: None,
                    correlation_id: None,
                },
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
                ordering: mutsuki_runtime_contracts::OrderingRequirement::PreserveSubmitOrder,
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
