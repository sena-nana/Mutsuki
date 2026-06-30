use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_MEDIA_UPLOAD_PROTOCOL_ID, BOT_MESSAGE_RECALL_PROTOCOL_ID,
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotMediaUploadRequest, BotMessage, BotMessageRecallRequest,
    QQBOT_ACCOUNT_GET_PROTOCOL_ID, QQBOT_GATEWAY_STATUS_PROTOCOL_ID, QQBOT_RAW_CALL_PROTOCOL_ID,
    QqBotAccountGetRequest, QqBotGatewayStatusRequest,
};
use mutsuki_runtime_contracts::{
    ERR_RUNTIME_HOST_FAILED, ExecutionClass, RunnerDescriptor, RunnerPurity, RunnerResult,
    RuntimeError, ScalarValue, Task,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use serde_json::{Value, json};

use crate::adapter::{
    bot_media_upload_to_qq_upload, bot_message_to_qq_send as map_bot_message_to_qq_send,
    bot_recall_to_qq_recall, qq_gateway_frame_to_bot_event, redact_json,
};
use crate::api::{
    QqBotClients, QqIdSource, QqOpenApiError, QqOpenApiService, RawCallPayload, parse_payload,
};
use crate::config::QqBotConfig;
use crate::gateway::GatewayFrame;
pub use crate::gateway::QQBOT_GATEWAY_FRAME_PROTOCOL_ID;

pub const QQBOT_ADAPTER_PLUGIN_ID: &str = "mutsuki.bot.adapter.qqbot";
pub const QQBOT_GATEWAY_RUNNER_ID: &str = "mutsuki.bot.adapter.qqbot.gateway";
pub const QQBOT_OPENAPI_RUNNER_ID: &str = "mutsuki.bot.adapter.qqbot.openapi";
pub const QQBOT_OPENAPI_RESULT_EVENT: &str = "mutsuki.bot.qqbot.openapi.result";

pub fn qqbot_runners(
    config: QqBotConfig,
    clients: QqBotClients,
    id_source: Box<dyn QqIdSource>,
) -> Vec<Box<dyn Runner>> {
    vec![
        Box::new(QqGatewayMapRunner::new(1, config.account_id.clone())),
        Box::new(QqOpenApiRunner::new(1, config, clients, id_source)),
    ]
}

pub struct QqGatewayMapRunner {
    descriptor: RunnerDescriptor,
    account_id: String,
}

impl QqGatewayMapRunner {
    pub fn new(plugin_generation: u64, account_id: impl Into<String>) -> Self {
        Self {
            descriptor: gateway_descriptor(plugin_generation),
            account_id: account_id.into(),
        }
    }
}

impl Runner for QqGatewayMapRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        tasks
            .into_iter()
            .map(|task| {
                let frame: GatewayFrame = serde_json::from_value(task.payload.clone())
                    .map_err(|error| failure("mutsuki.bot.qqbot.gateway.decode", error))?;
                let event = qq_gateway_frame_to_bot_event(&self.account_id, frame)
                    .map_err(|error| failure("mutsuki.bot.qqbot.gateway.map", error))?;
                let mut ingest = Task::new(
                    format!("mutsuki.bot.event.ingest:{}", task.task_id),
                    BOT_EVENT_INGEST_PROTOCOL_ID,
                    serde_json::to_value(event)
                        .map_err(|error| failure("mutsuki.bot.qqbot.gateway.encode", error))?,
                );
                ingest.registry_generation = ctx.registry_generation;
                let mut result = RunnerResult::completed(task.task_id);
                result.tasks.push(ingest);
                Ok(result)
            })
            .collect()
    }
}

pub struct QqOpenApiRunner {
    descriptor: RunnerDescriptor,
    service: QqOpenApiService,
}

impl QqOpenApiRunner {
    pub fn new(
        plugin_generation: u64,
        config: QqBotConfig,
        clients: QqBotClients,
        id_source: Box<dyn QqIdSource>,
    ) -> Self {
        Self {
            descriptor: openapi_descriptor(plugin_generation),
            service: QqOpenApiService::new(config, clients, id_source),
        }
    }
}

impl Runner for QqOpenApiRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        let mut results = Vec::new();
        for task in tasks {
            let response = match task.protocol_id.as_str() {
                BOT_MESSAGE_SEND_PROTOCOL_ID => {
                    let message: BotMessage = serde_json::from_value(task.payload.clone())
                        .map_err(|error| failure("mutsuki.bot.message.send.decode", error))?;
                    self.service.send_message(
                        map_bot_message_to_qq_send(message).map_err(|error| {
                            failure("mutsuki.bot.message.send.map.qqbot", error)
                        })?,
                        ctx.current_step,
                    )
                }
                BOT_MEDIA_UPLOAD_PROTOCOL_ID => {
                    let request: BotMediaUploadRequest = parse_payload(task.payload.clone())
                        .map_err(|error| failure("mutsuki.bot.media.upload.payload", error))?;
                    self.service.upload_media(
                        bot_media_upload_to_qq_upload(request).map_err(|error| {
                            failure("mutsuki.bot.media.upload.map.qqbot", error)
                        })?,
                        ctx.current_step,
                    )
                }
                BOT_MESSAGE_RECALL_PROTOCOL_ID => {
                    let request: BotMessageRecallRequest = parse_payload(task.payload.clone())
                        .map_err(|error| failure("mutsuki.bot.message.recall.payload", error))?;
                    self.service.recall_message(
                        bot_recall_to_qq_recall(request).map_err(|error| {
                            failure("mutsuki.bot.message.recall.map.qqbot", error)
                        })?,
                        ctx.current_step,
                    )
                }
                QQBOT_ACCOUNT_GET_PROTOCOL_ID => {
                    let _: QqBotAccountGetRequest = parse_payload(task.payload.clone())
                        .map_err(|error| failure("mutsuki.bot.qqbot.account.get.payload", error))?;
                    self.service.get_account(ctx.current_step)
                }
                QQBOT_GATEWAY_STATUS_PROTOCOL_ID => {
                    let _: QqBotGatewayStatusRequest = parse_payload(task.payload.clone())
                        .map_err(|error| {
                            failure("mutsuki.bot.qqbot.gateway.status.payload", error)
                        })?;
                    self.service.gateway_status(ctx.current_step)
                }
                QQBOT_RAW_CALL_PROTOCOL_ID => self.service.raw_call(
                    parse_payload::<RawCallPayload>(task.payload.clone())
                        .map_err(|error| failure("mutsuki.bot.qqbot.raw.call.payload", error))?,
                    ctx.current_step,
                ),
                _ => Err(QqOpenApiError::InvalidPayload(format!(
                    "unsupported task protocol {}",
                    task.protocol_id
                ))),
            }
            .map_err(|error| openapi_failure(&task.protocol_id, error))?;

            let mut result = RunnerResult::completed(task.task_id.clone());
            result.events.push(result_event(&task, response));
            results.push(result);
        }
        Ok(results)
    }
}

pub fn gateway_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: QQBOT_GATEWAY_RUNNER_ID.into(),
        plugin_id: QQBOT_ADAPTER_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![QQBOT_GATEWAY_FRAME_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Io,
        input_schema: json!({
            "type": "object",
            "required": ["op"]
        }),
        output_schema: json!({
            "tasks": [BOT_EVENT_INGEST_PROTOCOL_ID]
        }),
        metadata: metadata("QQBot Gateway frame mapper"),
        contract_surfaces: vec![
            format!("runner:{QQBOT_GATEWAY_RUNNER_ID}"),
            format!("task_protocol:{QQBOT_GATEWAY_FRAME_PROTOCOL_ID}"),
        ],
    }
}

pub fn openapi_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: QQBOT_OPENAPI_RUNNER_ID.into(),
        plugin_id: QQBOT_ADAPTER_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![
            BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
            BOT_MEDIA_UPLOAD_PROTOCOL_ID.into(),
            BOT_MESSAGE_RECALL_PROTOCOL_ID.into(),
            QQBOT_ACCOUNT_GET_PROTOCOL_ID.into(),
            QQBOT_GATEWAY_STATUS_PROTOCOL_ID.into(),
            QQBOT_RAW_CALL_PROTOCOL_ID.into(),
        ],
        purity: RunnerPurity::Effectful,
        execution_class: ExecutionClass::Blocking,
        input_schema: json!({
            "type": "object",
            "additionalProperties": true
        }),
        output_schema: json!({
            "events": [QQBOT_OPENAPI_RESULT_EVENT]
        }),
        metadata: metadata("QQBot OpenAPI adapter"),
        contract_surfaces: vec![format!("runner:{QQBOT_OPENAPI_RUNNER_ID}")],
    }
}

fn result_event(task: &Task, response: Value) -> mutsuki_runtime_contracts::DomainEvent {
    mutsuki_runtime_contracts::DomainEvent {
        event_id: format!("{}:result", task.task_id),
        kind: QQBOT_OPENAPI_RESULT_EVENT.into(),
        payload: json!({
            "task_protocol": task.protocol_id,
            "response": response,
        }),
    }
}

fn metadata(description: &str) -> BTreeMap<String, ScalarValue> {
    BTreeMap::from([
        (
            "description".into(),
            ScalarValue::String(description.into()),
        ),
        ("domain".into(), ScalarValue::String("bot.qqbot".into())),
    ])
}

fn failure(route: impl Into<String>, error: impl std::fmt::Display) -> RuntimeFailure {
    let mut runtime_error = RuntimeError::new(
        ERR_RUNTIME_HOST_FAILED,
        QQBOT_ADAPTER_PLUGIN_ID,
        route.into(),
    );
    runtime_error
        .evidence
        .insert("message".into(), ScalarValue::String(error.to_string()));
    RuntimeFailure::new(runtime_error)
}

fn openapi_failure(route: &str, error: QqOpenApiError) -> RuntimeFailure {
    let mut runtime_error =
        RuntimeError::new(ERR_RUNTIME_HOST_FAILED, QQBOT_ADAPTER_PLUGIN_ID, route);
    runtime_error.evidence = BTreeMap::from([(
        "message".into(),
        ScalarValue::String(error.redacted_message()),
    )]);
    if let QqOpenApiError::HttpStatus { body, .. } = error {
        runtime_error.evidence.insert(
            "body".into(),
            ScalarValue::String(redact_json(&body).to_string()),
        );
    }
    RuntimeFailure::new(runtime_error)
}
