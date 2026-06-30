use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_COMMAND_HANDLE_PROTOCOL_ID, BOT_COMMAND_PARSE_PROTOCOL_ID, BotCommandEvent, BotEvent,
};
use mutsuki_runtime_contracts::{
    ERR_RUNTIME_HOST_FAILED, ExecutionClass, RunnerDescriptor, RunnerPurity, RunnerResult,
    RuntimeError, ScalarValue, Task,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use serde_json::json;

use crate::{CommandParseError, CommandParser, message_text};

pub const BOT_COMMAND_PLUGIN_ID: &str = "mutsuki.bot.command";
pub const BOT_COMMAND_RUNNER_ID: &str = "mutsuki.bot.command.parse";

pub struct BotCommandRunner {
    descriptor: RunnerDescriptor,
    parser: CommandParser,
}

impl BotCommandRunner {
    pub fn new(plugin_generation: u64, prefixes: Vec<String>) -> Self {
        Self {
            descriptor: command_descriptor(plugin_generation),
            parser: CommandParser::new(prefixes),
        }
    }
}

impl Runner for BotCommandRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        tasks
            .into_iter()
            .map(|task| {
                let event: BotEvent = serde_json::from_value(task.payload.clone())
                    .map_err(|error| failure("mutsuki.bot.command.decode", error))?;
                let Some(text) = message_text(&event) else {
                    return Ok(RunnerResult::completed(task.task_id));
                };
                let command = match self.parser.parse(&text) {
                    Ok(command) => command,
                    Err(CommandParseError::MissingPrefix) => {
                        return Ok(RunnerResult::completed(task.task_id));
                    }
                    Err(error) => return Err(failure("mutsuki.bot.command.parse", error)),
                };
                let command_event = BotCommandEvent {
                    source: event,
                    name: command.name,
                    args: command.args,
                    raw_text: command.raw_text,
                };
                let mut child = Task::new(
                    format!("mutsuki.bot.command.handle:{}", task.task_id),
                    BOT_COMMAND_HANDLE_PROTOCOL_ID,
                    serde_json::to_value(command_event)
                        .map_err(|error| failure("mutsuki.bot.command.encode", error))?,
                );
                child.registry_generation = ctx.registry_generation;
                let mut result = RunnerResult::completed(task.task_id);
                result.tasks.push(child);
                Ok(result)
            })
            .collect()
    }
}

pub fn command_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: BOT_COMMAND_RUNNER_ID.into(),
        plugin_id: BOT_COMMAND_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![BOT_COMMAND_PARSE_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Orchestration,
        input_schema: json!({
            "type": "object",
            "required": ["event_id", "message"]
        }),
        output_schema: json!({
            "tasks": [BOT_COMMAND_HANDLE_PROTOCOL_ID]
        }),
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

fn failure(route: impl Into<String>, error: impl std::fmt::Display) -> RuntimeFailure {
    let mut runtime_error =
        RuntimeError::new(ERR_RUNTIME_HOST_FAILED, BOT_COMMAND_PLUGIN_ID, route.into());
    runtime_error
        .evidence
        .insert("message".into(), ScalarValue::String(error.to_string()));
    RuntimeFailure::new(runtime_error)
}
