use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_MEDIA_UPLOAD_PROTOCOL_ID, BOT_MESSAGE_RECALL_PROTOCOL_ID,
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotMediaUploadRequest, BotMessage, BotMessageRecallRequest,
    QQBOT_ACCOUNT_GET_PROTOCOL_ID, QQBOT_GATEWAY_STATUS_PROTOCOL_ID, QQBOT_RAW_CALL_PROTOCOL_ID,
    QqBotAccountGetRequest, QqBotGatewayStatusRequest,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ERR_RUNTIME_HOST_FAILED, ExecutionClass, InvocationMode, OrderingRequirement,
    PluginManifest, RunnerBatchCapability, RunnerConcurrency, RunnerControlCapability,
    RunnerDescriptor, RunnerMode, RunnerOrderingCapability, RunnerPayloadCapability, RunnerPurity,
    RunnerResourceCapability, RunnerResult, RunnerSideEffect, RuntimeError, ScalarValue, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{PluginBuilder, map_work_batch_entries};
use serde_json::{Value, json};

use crate::adapter::{
    bot_media_upload_to_qq_upload, bot_recall_to_qq_recall, qq_gateway_frame_to_bot_event,
    redact_json,
};
use crate::api::{
    QqAuthManager, QqBotClients, QqIdSource, QqOpenApiError, QqOpenApiService, RawCallPayload,
    parse_payload,
};
use crate::config::QqBotConfig;
use crate::gateway::GatewayFrame;
pub use crate::gateway::QQBOT_GATEWAY_FRAME_PROTOCOL_ID;

pub const QQBOT_ADAPTER_PLUGIN_ID: &str = "mutsuki.bot.adapter.qqbot";
pub const QQBOT_GATEWAY_RUNNER_ID: &str = "mutsuki.bot.adapter.qqbot.gateway";
pub const QQBOT_OPENAPI_RUNNER_ID: &str = "mutsuki.bot.adapter.qqbot.openapi";
pub const QQBOT_OPENAPI_RESULT_EVENT: &str = "mutsuki.bot.qqbot.openapi.result";

pub fn qqbot_adapter_manifest(plugin_generation: u64, media_enabled: bool) -> PluginManifest {
    PluginBuilder::new(QQBOT_ADAPTER_PLUGIN_ID)
        .metadata("platform", ScalarValue::String("qqbot".into()))
        .metadata("adapter", ScalarValue::Bool(true))
        .runner_descriptor(gateway_descriptor(plugin_generation))
        .runner_descriptor(openapi_descriptor(plugin_generation, media_enabled))
        .build()
        .manifest
}

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

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let frame: GatewayFrame = serde_json::from_value(task.payload.clone().into())
                .map_err(|error| failure("mutsuki.bot.qqbot.gateway.decode", error))?;
            let event = qq_gateway_frame_to_bot_event(&self.account_id, frame)
                .map_err(|error| failure("mutsuki.bot.qqbot.gateway.map", error))?;
            tracing::info!(
                account_id = %self.account_id,
                event_id = %event.event_id,
                task_id = %task.task_id,
                runner_id = QQBOT_GATEWAY_RUNNER_ID,
                correlation_id = task.correlation_id.as_deref().unwrap_or(""),
                "QQBot Gateway event mapped"
            );
            let mut ingest = Task::new(
                format!("mutsuki.bot.event.ingest:{}", task.task_id),
                BOT_EVENT_INGEST_PROTOCOL_ID,
                mutsuki_runtime_contracts::TaskPayload::from_local(event),
            );
            ingest.registry_generation = ctx.registry_generation;
            ingest.trace_id = task.trace_id.clone();
            ingest.correlation_id = task.correlation_id.clone();
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.tasks.push(ingest);
            Ok(result)
        })
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
        let media_enabled = clients.has_media_provider();
        Self {
            descriptor: openapi_descriptor(plugin_generation, media_enabled),
            service: QqOpenApiService::new(config, clients, id_source),
        }
    }

    pub fn new_with_auth(
        plugin_generation: u64,
        config: QqBotConfig,
        clients: QqBotClients,
        id_source: Box<dyn QqIdSource>,
        auth: QqAuthManager,
    ) -> Self {
        let media_enabled = clients.has_media_provider();
        Self {
            descriptor: openapi_descriptor(plugin_generation, media_enabled),
            service: QqOpenApiService::new_with_auth(config, clients, id_source, auth),
        }
    }
}

impl Runner for QqOpenApiRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let account_id = self.service.account_id().to_owned();
        map_work_batch_entries(&batch, |task| {
            let response = match task.protocol_id.as_str() {
                BOT_MESSAGE_SEND_PROTOCOL_ID => {
                    let message: BotMessage =
                        serde_json::from_value(task.payload.clone().into())
                            .map_err(|error| failure("mutsuki.bot.message.send.decode", error))?;
                    self.service.send_bot_message(message)
                }
                BOT_MEDIA_UPLOAD_PROTOCOL_ID => {
                    let request: BotMediaUploadRequest = parse_payload(task.payload.clone().into())
                        .map_err(|error| failure("mutsuki.bot.media.upload.payload", error))?;
                    self.service.upload_media(
                        bot_media_upload_to_qq_upload(request).map_err(|error| {
                            failure("mutsuki.bot.media.upload.map.qqbot", error)
                        })?,
                    )
                }
                BOT_MESSAGE_RECALL_PROTOCOL_ID => {
                    let request: BotMessageRecallRequest =
                        parse_payload(task.payload.clone().into()).map_err(|error| {
                            failure("mutsuki.bot.message.recall.payload", error)
                        })?;
                    self.service.recall_message(
                        bot_recall_to_qq_recall(request).map_err(|error| {
                            failure("mutsuki.bot.message.recall.map.qqbot", error)
                        })?,
                    )
                }
                QQBOT_ACCOUNT_GET_PROTOCOL_ID => {
                    let _: QqBotAccountGetRequest = parse_payload(task.payload.clone().into())
                        .map_err(|error| failure("mutsuki.bot.qqbot.account.get.payload", error))?;
                    self.service.get_account()
                }
                QQBOT_GATEWAY_STATUS_PROTOCOL_ID => {
                    let _: QqBotGatewayStatusRequest = parse_payload(task.payload.clone().into())
                        .map_err(|error| {
                        failure("mutsuki.bot.qqbot.gateway.status.payload", error)
                    })?;
                    self.service.gateway_status()
                }
                QQBOT_RAW_CALL_PROTOCOL_ID => self.service.raw_call(
                    parse_payload::<RawCallPayload>(task.payload.clone().into())
                        .map_err(|error| failure("mutsuki.bot.qqbot.raw.call.payload", error))?,
                ),
                _ => Err(QqOpenApiError::InvalidPayload(format!(
                    "unsupported task protocol {}",
                    task.protocol_id
                ))),
            }
            .map_err(|error| openapi_failure(&task.protocol_id, error))?;

            tracing::info!(
                account_id = %account_id,
                task_id = %task.task_id,
                runner_id = QQBOT_OPENAPI_RUNNER_ID,
                protocol_id = %task.protocol_id,
                correlation_id = task.correlation_id.as_deref().unwrap_or(""),
                reply_request_id = %task.task_id,
                "QQBot OpenAPI request completed"
            );

            let mut result = RunnerResult::completed(task.task_id.clone());
            result.events.push(result_event(task, response));
            Ok(result)
        })
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
        invocation_mode: InvocationMode::SyncExclusive,
        concurrency: RunnerConcurrency::Exclusive,
        input_schema: json!({
            "type": "object",
            "required": ["op"]
        }),
        output_schema: json!({
            "tasks": [BOT_EVENT_INGEST_PROTOCOL_ID]
        }),
        batch: native_batch_capability(RunnerSideEffect::None, 16, 64),
        payload: RunnerPayloadCapability::default(),
        resources: resource_capability(),
        ordering: preserve_submit_order(),
        control: RunnerControlCapability::default(),
        metadata: metadata("QQBot Gateway frame mapper"),
        contract_surfaces: vec![
            format!("runner:{QQBOT_GATEWAY_RUNNER_ID}"),
            format!("task_protocol:{QQBOT_GATEWAY_FRAME_PROTOCOL_ID}"),
        ],
    }
}

pub fn openapi_descriptor(plugin_generation: u64, media_enabled: bool) -> RunnerDescriptor {
    let mut accepted_protocol_ids = vec![
        BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
        BOT_MESSAGE_RECALL_PROTOCOL_ID.into(),
        QQBOT_ACCOUNT_GET_PROTOCOL_ID.into(),
        QQBOT_GATEWAY_STATUS_PROTOCOL_ID.into(),
        QQBOT_RAW_CALL_PROTOCOL_ID.into(),
    ];
    if media_enabled {
        accepted_protocol_ids.push(BOT_MEDIA_UPLOAD_PROTOCOL_ID.into());
    }
    RunnerDescriptor {
        runner_id: QQBOT_OPENAPI_RUNNER_ID.into(),
        plugin_id: QQBOT_ADAPTER_PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids,
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Blocking,
        invocation_mode: InvocationMode::SyncExclusive,
        concurrency: RunnerConcurrency::Exclusive,
        input_schema: json!({
            "type": "object",
            "additionalProperties": true
        }),
        output_schema: json!({
            "events": [QQBOT_OPENAPI_RESULT_EVENT]
        }),
        batch: native_batch_capability(RunnerSideEffect::External, 1, 32),
        payload: RunnerPayloadCapability::default(),
        resources: resource_capability(),
        ordering: preserve_submit_order(),
        control: RunnerControlCapability::default(),
        metadata: metadata("QQBot OpenAPI adapter"),
        contract_surfaces: vec![format!("runner:{QQBOT_OPENAPI_RUNNER_ID}")],
    }
}

fn result_event(task: &Task, response: Value) -> mutsuki_runtime_contracts::DomainEvent {
    mutsuki_runtime_contracts::DomainEvent {
        event_id: format!("{}:result", task.task_id),
        kind: QQBOT_OPENAPI_RESULT_EVENT.into(),
        payload: json!({
            "task_id": task.task_id,
            "protocol_id": task.protocol_id,
            "response": response,
        }),
    }
}

fn native_batch_capability(
    side_effect: RunnerSideEffect,
    preferred_batch_size: usize,
    max_batch_entries: usize,
) -> RunnerBatchCapability {
    RunnerBatchCapability {
        mode: RunnerMode::NativeBatch,
        preferred_batch_size,
        max_batch_entries,
        preserve_order: true,
        side_effect,
        ..Default::default()
    }
}

fn resource_capability() -> RunnerResourceCapability {
    RunnerResourceCapability {
        requires_resource_plan: false,
        ..Default::default()
    }
}

fn preserve_submit_order() -> RunnerOrderingCapability {
    RunnerOrderingCapability {
        default: OrderingRequirement::PreserveSubmitOrder,
        supports_sequence: true,
        supports_same_resource_order: true,
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

fn failure(route: impl Into<String>, error: impl std::fmt::Display) -> RuntimeError {
    let mut runtime_error = RuntimeError::new(
        ERR_RUNTIME_HOST_FAILED,
        QQBOT_ADAPTER_PLUGIN_ID,
        route.into(),
    );
    runtime_error
        .evidence
        .insert("message".into(), ScalarValue::String(error.to_string()));
    runtime_error
}

fn openapi_failure(route: &str, error: QqOpenApiError) -> RuntimeError {
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
    runtime_error
}
