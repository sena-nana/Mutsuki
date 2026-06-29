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
    EFFECT_USER_SHARE_LINK, OPENAPI_RESULT_EVENT, PLUGIN_ID, gateway_normalizer_descriptor,
    openapi_effect_descriptor,
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

const GATEWAY_PARSE_ROUTE: &str = "mutsuki.im.qqbot.gateway.parse";
const GATEWAY_NORMALIZE_ROUTE: &str = "mutsuki.im.qqbot.gateway.normalize";

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
                    .map_err(|error| runtime_failure(GATEWAY_PARSE_ROUTE, error.to_string()))?;
                let events = normalize_gateway_frame(frame)
                    .map_err(|error| runtime_failure(GATEWAY_NORMALIZE_ROUTE, error))?;
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
                    parse_task_payload::<SendMessagePayload>(&task, "message.send")?,
                    ctx.current_step,
                ),
                EFFECT_MEDIA_UPLOAD => self.service.upload_media(
                    parse_task_payload::<MediaUploadPayload>(&task, "media.upload")?,
                    ctx.current_step,
                ),
                EFFECT_MESSAGE_RECALL => self.service.recall_message(
                    parse_task_payload::<RecallMessagePayload>(&task, "message.recall")?,
                    ctx.current_step,
                ),
                EFFECT_INTERACTION_ACK => self.service.ack_interaction(
                    parse_task_payload::<InteractionAckPayload>(&task, "interaction.ack")?,
                    ctx.current_step,
                ),
                EFFECT_USER_SHARE_LINK => self.service.create_user_share_link(
                    parse_task_payload::<UserShareLinkPayload>(&task, "user.share_link")?,
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
        kind: OPENAPI_RESULT_EVENT.into(),
        payload: json!({
            "task_protocol": task.protocol_id,
            "response": response,
        }),
    }
}

fn parse_task_payload<T: serde::de::DeserializeOwned>(
    task: &Task,
    route_suffix: &str,
) -> RuntimeResult<T> {
    parse_payload::<T>(task.payload.clone()).map_err(|error| {
        runtime_failure(
            format!("mutsuki.im.qqbot.{route_suffix}.payload"),
            error.to_string(),
        )
    })
}

fn runtime_failure(route: impl Into<String>, message: impl Into<String>) -> RuntimeFailure {
    let message = message.into();
    let mut error = RuntimeError::new(ERR_RUNTIME_HOST_FAILED, PLUGIN_ID, route);
    error
        .evidence
        .insert("message".into(), ScalarValue::String(message));
    RuntimeFailure::new(error)
}

fn openapi_failure(route: &str, error: QqOpenApiError) -> RuntimeFailure {
    let mut runtime_error = RuntimeError::new(ERR_RUNTIME_HOST_FAILED, PLUGIN_ID, route);
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
