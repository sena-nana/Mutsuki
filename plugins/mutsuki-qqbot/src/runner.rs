use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    DomainEvent, ERR_RUNTIME_HOST_FAILED, RunnerDescriptor, RunnerResult, RuntimeError,
    ScalarValue, Task,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use serde_json::{Value, json};

use crate::config::QqBotConfig;
use crate::gateway::{GatewayFrame, normalize_gateway_frame};
use crate::manifest::{
    EFFECT_INTERACTION_ACK, EFFECT_MEDIA_UPLOAD, EFFECT_MESSAGE_RECALL, EFFECT_MESSAGE_SEND,
    EFFECT_USER_SHARE_LINK, gateway_normalizer_descriptor, openapi_effect_descriptor,
};
use crate::openapi::{QqBotClients, QqIdSource, QqOpenApiError, QqOpenApiService};
use crate::payload::{
    InteractionAckPayload, MediaUploadPayload, RecallMessagePayload, SendMessagePayload,
    UserShareLinkPayload, parse_payload,
};
use crate::redaction::redact_json;

pub fn qqbot_runners(
    config: QqBotConfig,
    clients: QqBotClients,
    id_source: Box<dyn QqIdSource>,
) -> Vec<Box<dyn Runner>> {
    vec![
        Box::new(QqGatewayNormalizeRunner::new(1)),
        Box::new(QqOpenApiRunner::new(1, config, clients, id_source)),
    ]
}

pub struct QqGatewayNormalizeRunner {
    descriptor: RunnerDescriptor,
}

impl QqGatewayNormalizeRunner {
    pub fn new(plugin_generation: u64) -> Self {
        Self {
            descriptor: gateway_normalizer_descriptor(plugin_generation),
        }
    }
}

impl Runner for QqGatewayNormalizeRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, _ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        tasks
            .into_iter()
            .map(|task| {
                let frame: GatewayFrame = serde_json::from_value(task.payload.clone())
                    .map_err(|error| runtime_failure("qqbot.gateway.parse", error.to_string()))?;
                let events = normalize_gateway_frame(frame)
                    .map_err(|error| runtime_failure("qqbot.gateway.normalize", error))?;
                let mut result = RunnerResult::completed(task.task_id);
                result.events = events;
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
            descriptor: openapi_effect_descriptor(plugin_generation),
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
                EFFECT_MESSAGE_SEND => self.service.send_message(
                    parse_payload::<SendMessagePayload>(task.payload.clone()).map_err(|error| {
                        runtime_failure("qqbot.message.send.payload", error.to_string())
                    })?,
                    ctx.current_step,
                ),
                EFFECT_MEDIA_UPLOAD => self.service.upload_media(
                    parse_payload::<MediaUploadPayload>(task.payload.clone()).map_err(|error| {
                        runtime_failure("qqbot.media.upload.payload", error.to_string())
                    })?,
                    ctx.current_step,
                ),
                EFFECT_MESSAGE_RECALL => self.service.recall_message(
                    parse_payload::<RecallMessagePayload>(task.payload.clone()).map_err(
                        |error| runtime_failure("qqbot.message.recall.payload", error.to_string()),
                    )?,
                    ctx.current_step,
                ),
                EFFECT_INTERACTION_ACK => self.service.ack_interaction(
                    parse_payload::<InteractionAckPayload>(task.payload.clone()).map_err(
                        |error| runtime_failure("qqbot.interaction.ack.payload", error.to_string()),
                    )?,
                    ctx.current_step,
                ),
                EFFECT_USER_SHARE_LINK => self.service.create_user_share_link(
                    parse_payload::<UserShareLinkPayload>(task.payload.clone()).map_err(
                        |error| runtime_failure("qqbot.user.share_link.payload", error.to_string()),
                    )?,
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

fn result_event(task: &Task, response: Value) -> DomainEvent {
    DomainEvent {
        event_id: format!("{}:result", task.task_id),
        kind: "qqbot.openapi.result".into(),
        payload: json!({
            "task_protocol": task.protocol_id,
            "response": response,
        }),
    }
}

fn runtime_failure(route: impl Into<String>, message: impl Into<String>) -> RuntimeFailure {
    let message = message.into();
    let mut error = RuntimeError::new(ERR_RUNTIME_HOST_FAILED, "mutsuki.qqbot", route);
    error
        .evidence
        .insert("message".into(), ScalarValue::String(message));
    RuntimeFailure::new(error)
}

fn openapi_failure(route: &str, error: QqOpenApiError) -> RuntimeFailure {
    let mut runtime_error = RuntimeError::new(ERR_RUNTIME_HOST_FAILED, "mutsuki.qqbot", route);
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
