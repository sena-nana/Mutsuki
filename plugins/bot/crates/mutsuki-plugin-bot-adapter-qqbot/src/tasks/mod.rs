use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE, BOT_FLOW_INGRESS_PROTOCOL_ID,
    BOT_MEDIA_UPLOAD_PROTOCOL_ID, BOT_MESSAGE_RECALL_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BotEvent, BotFlowContext, BotFlowEventEnvelope, BotFlowPayload, BotFlowTypeRef,
    BotMediaUploadRequest, BotMessage, BotMessageRecallRequest, BotNodeBinding,
    BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodePortDescriptor,
    BotNodePortDirection, BotNodeResult, BotNodeRole, MessageSegment,
    QQBOT_ACCOUNT_GET_PROTOCOL_ID, QQBOT_CAPABILITY_GET_PROTOCOL_ID,
    QQBOT_GATEWAY_STATUS_PROTOCOL_ID, QQBOT_RAW_CALL_PROTOCOL_ID, QqBotAccountGetRequest,
    QqBotCapabilityGetRequest, QqBotGatewayStatusRequest,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ERR_RUNTIME_HOST_FAILED, ExecutionClass, InvocationMode, OrderingRequirement,
    PluginManifest, RunnerBatchCapability, RunnerConcurrency, RunnerControlCapability,
    RunnerDescriptor, RunnerMode, RunnerOrderingCapability, RunnerPayloadCapability, RunnerPurity,
    RunnerResourceCapability, RunnerResult, RunnerSideEffect, RuntimeError, ScalarValue, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{PluginBuilder, ProtocolDescriptorBuilder, map_work_batch_entries};
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
use crate::inbound_media::gateway_media_descriptor;

pub const QQBOT_ADAPTER_PLUGIN_ID: &str = "mutsuki.bot.adapter.qqbot";
pub const QQBOT_GATEWAY_RUNNER_ID: &str = "mutsuki.bot.adapter.qqbot.gateway";
pub const QQBOT_OPENAPI_RUNNER_ID: &str = "mutsuki.bot.adapter.qqbot.openapi";
pub const QQBOT_OPENAPI_RESULT_EVENT: &str = "mutsuki.bot.qqbot.openapi.result";

pub fn qqbot_adapter_manifest(plugin_generation: u64, media_enabled: bool) -> PluginManifest {
    let gateway = if media_enabled {
        gateway_media_descriptor(plugin_generation)
    } else {
        gateway_descriptor(plugin_generation)
    };
    let mut builder = PluginBuilder::new(QQBOT_ADAPTER_PLUGIN_ID)
        .metadata("platform", ScalarValue::String("qqbot".into()))
        .metadata("adapter", ScalarValue::Bool(true))
        .runner_descriptor(gateway)
        .runner_descriptor(openapi_descriptor(plugin_generation, media_enabled))
        .protocol_handler(
            qqbot_protocol_descriptor(
                BOT_MESSAGE_SEND_PROTOCOL_ID,
                &["target", "segments"],
                &["message_id"],
            ),
            QQBOT_OPENAPI_RUNNER_ID,
            "qqbot-message-send",
        )
        .protocol_handler(
            qqbot_protocol_descriptor(
                BOT_MESSAGE_RECALL_PROTOCOL_ID,
                &["target", "message_id"],
                &["message_id"],
            ),
            QQBOT_OPENAPI_RUNNER_ID,
            "qqbot-message-recall",
        );
    if media_enabled {
        builder = builder.protocol_handler(
            qqbot_protocol_descriptor(
                BOT_MEDIA_UPLOAD_PROTOCOL_ID,
                &["target", "resource"],
                &["file_info"],
            ),
            QQBOT_OPENAPI_RUNNER_ID,
            "qqbot-media-upload",
        );
    }
    builder
        .extension(
            qqbot_node_catalog(media_enabled)
                .into_plugin_extension()
                .expect("QQBot node catalog serializes"),
        )
        .build()
        .manifest
}

fn qqbot_protocol_descriptor(
    protocol_id: &str,
    request_required: &[&str],
    response_required: &[&str],
) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object", "required": request_required}))
        .output_schema(json!({"type": "object", "required": response_required}))
        .error_schema(json!({
            "type": "object",
            "required": ["code", "source", "route"]
        }))
        .build()
}

pub const QQ_NODE_MESSAGE_CREATED: &str = "mutsuki.bot.qq.message.created";
pub const QQ_NODE_MESSAGE_UPDATED: &str = "mutsuki.bot.qq.message.updated";
pub const QQ_NODE_MESSAGE_DELETED: &str = "mutsuki.bot.qq.message.deleted";
pub const QQ_NODE_REACTION_ADDED: &str = "mutsuki.bot.qq.reaction.added";
pub const QQ_NODE_REACTION_REMOVED: &str = "mutsuki.bot.qq.reaction.removed";
pub const QQ_NODE_MEMBER_JOINED: &str = "mutsuki.bot.qq.member.joined";
pub const QQ_NODE_MEMBER_LEFT: &str = "mutsuki.bot.qq.member.left";
pub const QQ_NODE_BOT_CONNECTED: &str = "mutsuki.bot.qq.bot.connected";
pub const QQ_NODE_BOT_DISCONNECTED: &str = "mutsuki.bot.qq.bot.disconnected";
pub const QQ_NODE_PLATFORM: &str = "mutsuki.bot.qq.platform";

fn qqbot_node_catalog(media_enabled: bool) -> BotNodeCatalogFragment {
    use mutsuki_bot_protocol::{
        BOT_FLOW_LIFECYCLE_EVENT_TYPE, BOT_FLOW_MEMBER_EVENT_TYPE,
        BOT_FLOW_MESSAGE_DELETED_EVENT_TYPE, BOT_FLOW_MESSAGE_EVENT_TYPE,
        BOT_FLOW_PLATFORM_EVENT_TYPE, BOT_FLOW_REACTION_EVENT_TYPE,
    };
    let mut nodes = vec![
        qq_source(
            QQ_NODE_MESSAGE_CREATED,
            "收到消息",
            BOT_FLOW_MESSAGE_EVENT_TYPE,
        ),
        qq_source(
            QQ_NODE_MESSAGE_UPDATED,
            "消息更新",
            BOT_FLOW_MESSAGE_EVENT_TYPE,
        ),
        qq_source(
            QQ_NODE_MESSAGE_DELETED,
            "消息删除",
            BOT_FLOW_MESSAGE_DELETED_EVENT_TYPE,
        ),
        qq_source(
            QQ_NODE_REACTION_ADDED,
            "添加表情",
            BOT_FLOW_REACTION_EVENT_TYPE,
        ),
        qq_source(
            QQ_NODE_REACTION_REMOVED,
            "取消表情",
            BOT_FLOW_REACTION_EVENT_TYPE,
        ),
        qq_source(
            QQ_NODE_MEMBER_JOINED,
            "成员加入",
            BOT_FLOW_MEMBER_EVENT_TYPE,
        ),
        qq_source(QQ_NODE_MEMBER_LEFT, "成员离开", BOT_FLOW_MEMBER_EVENT_TYPE),
        qq_source(
            QQ_NODE_BOT_CONNECTED,
            "机器人上线",
            BOT_FLOW_LIFECYCLE_EVENT_TYPE,
        ),
        qq_source(
            QQ_NODE_BOT_DISCONNECTED,
            "机器人下线",
            BOT_FLOW_LIFECYCLE_EVENT_TYPE,
        ),
        qq_source(QQ_NODE_PLATFORM, "平台事件", BOT_FLOW_PLATFORM_EVENT_TYPE),
    ];
    nodes.push(BotNodeDescriptor {
        node_type_id: "mutsuki.bot.qq.send".into(),
        version: 1,
        title: "发送消息".into(),
        category: "QQ".into(),
        role: BotNodeRole::Sink,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{BOT_MESSAGE_SEND_PROTOCOL_ID}"),
            protocol_id: BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
            runner_hint: Some(QQBOT_OPENAPI_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "input".into(),
                title: "消息".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                required: false,
            },
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "消息事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(
                    mutsuki_bot_protocol::BOT_FLOW_MESSAGE_EVENT_TYPE,
                    1,
                ),
                required: false,
            },
        ],
        config_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "text": {
                    "type": "string",
                    "title": "消息内容",
                    "description": "留空则发送上游内容"
                },
                "reply": {
                    "type": "boolean",
                    "title": "回复原消息",
                    "default": false
                }
            }
        }),
    });
    nodes.push(sink_node(
        "mutsuki.bot.qq.recall",
        "撤回消息",
        BOT_MESSAGE_RECALL_PROTOCOL_ID,
        "mutsuki.bot.message.recall",
    ));
    if media_enabled {
        nodes.push(sink_node(
            "mutsuki.bot.qq.media.upload",
            "上传媒体",
            BOT_MEDIA_UPLOAD_PROTOCOL_ID,
            "mutsuki.bot.media.upload",
        ));
    }
    BotNodeCatalogFragment { nodes }
}

fn sink_node(
    node_type_id: &str,
    title: &str,
    protocol_id: &str,
    event_type: &str,
) -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: node_type_id.into(),
        version: 1,
        title: title.into(),
        category: "QQ".into(),
        role: BotNodeRole::Sink,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{protocol_id}"),
            protocol_id: protocol_id.into(),
            runner_hint: Some(QQBOT_OPENAPI_RUNNER_ID.into()),
        }),
        ports: vec![BotNodePortDescriptor {
            port_id: "input".into(),
            title: "输入".into(),
            direction: BotNodePortDirection::Input,
            event_type: BotFlowTypeRef::new(event_type, 1),
            required: true,
        }],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    }
}

fn qq_source(node_type_id: &str, title: &str, output_type: &str) -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: node_type_id.into(),
        version: 1,
        title: title.into(),
        category: "QQ".into(),
        role: BotNodeRole::Source,
        binding: None,
        ports: vec![BotNodePortDescriptor {
            port_id: "event".into(),
            title: title.into(),
            direction: BotNodePortDirection::Output,
            event_type: BotFlowTypeRef::new(output_type, 1),
            required: false,
        }],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    }
}

pub fn qqbot_runners(
    config: QqBotConfig,
    clients: QqBotClients,
    id_source: Box<dyn QqIdSource>,
) -> Vec<Box<dyn Runner>> {
    vec![
        Box::new(QqGatewayMapRunner::with_app_id(
            1,
            config.account_id.clone(),
            config.app_id.clone(),
        )),
        Box::new(QqOpenApiRunner::new(1, config, clients, id_source)),
    ]
}

pub struct QqGatewayMapRunner {
    descriptor: RunnerDescriptor,
    account_id: String,
    app_id: String,
}

impl QqGatewayMapRunner {
    pub fn new(plugin_generation: u64, account_id: impl Into<String>) -> Self {
        Self::with_app_id(plugin_generation, account_id, "")
    }

    pub fn with_app_id(
        plugin_generation: u64,
        account_id: impl Into<String>,
        app_id: impl Into<String>,
    ) -> Self {
        Self {
            descriptor: gateway_descriptor(plugin_generation),
            account_id: account_id.into(),
            app_id: app_id.into(),
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
            let event = qq_gateway_frame_to_bot_event(&self.account_id, &self.app_id, frame)
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
                format!("mutsuki.bot.flow.ingress:{}", task.task_id),
                BOT_FLOW_INGRESS_PROTOCOL_ID,
                mutsuki_runtime_contracts::TaskPayload::from_local(flow_envelope(
                    event,
                    task.trace_id.clone().map(Into::into),
                    task.correlation_id.clone(),
                )?),
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
            let (payload, invocation) = node_payload(task)?;
            let response = match task.protocol_id.as_str() {
                BOT_MESSAGE_SEND_PROTOCOL_ID => {
                    let message = message_from_node_payload(&payload, invocation.as_ref())
                        .map_err(|error| failure("mutsuki.bot.message.send.decode", error))?;
                    self.service.send_bot_message(message)
                }
                BOT_MEDIA_UPLOAD_PROTOCOL_ID => {
                    let request: BotMediaUploadRequest = parse_payload(payload.clone())
                        .map_err(|error| failure("mutsuki.bot.media.upload.payload", error))?;
                    self.service.upload_media(
                        bot_media_upload_to_qq_upload(request).map_err(|error| {
                            failure("mutsuki.bot.media.upload.map.qqbot", error)
                        })?,
                    )
                }
                BOT_MESSAGE_RECALL_PROTOCOL_ID => {
                    let request: BotMessageRecallRequest = parse_payload(payload.clone())
                        .map_err(|error| failure("mutsuki.bot.message.recall.payload", error))?;
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
                QQBOT_CAPABILITY_GET_PROTOCOL_ID => {
                    let _: QqBotCapabilityGetRequest = parse_payload(task.payload.clone().into())
                        .map_err(|error| {
                        failure("mutsuki.bot.qqbot.capability.get.payload", error)
                    })?;
                    serde_json::to_value(self.service.config().capability_matrix())
                        .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))
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
            .map_err(|error| openapi_failure(task.protocol_id.as_str(), error))?;

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
            result.output = Some(if invocation.is_some() {
                serde_json::to_value(BotNodeResult {
                    outputs: Vec::new(),
                    metadata: BTreeMap::from([("receipt".into(), response.clone())]),
                })
                .map_err(|error| failure("mutsuki.bot.qqbot.node.result", error))?
            } else {
                response.clone()
            });
            result.events.push(result_event(task, response));
            Ok(result)
        })
    }
}

fn node_payload(task: &Task) -> Result<(Value, Option<BotNodeInvocation>), RuntimeError> {
    let value = task.payload.to_value();
    match serde_json::from_value::<BotNodeInvocation>(value.clone()) {
        Ok(invocation) => Ok((invocation.input.payload.value.clone(), Some(invocation))),
        Err(_) => Ok((value, None)),
    }
}

fn message_from_node_payload(
    payload: &Value,
    invocation: Option<&BotNodeInvocation>,
) -> Result<BotMessage, serde_json::Error> {
    let text = invocation
        .and_then(|item| item.config.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let reply = invocation
        .and_then(|item| item.config.get("reply"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Ok(mut message) = serde_json::from_value::<BotMessage>(payload.clone()) {
        if let Some(text) = text {
            message.segments = vec![MessageSegment::text(text)];
        }
        if reply && message.reply_to.is_none() {
            message.reply_to = incoming_reply_to(invocation);
        }
        return Ok(message);
    }
    let event: BotEvent = serde_json::from_value(payload.clone())?;
    let body = text
        .or_else(|| event.message.as_ref().map(BotMessage::plain_text))
        .unwrap_or_default();
    let mut message = BotMessage::text(event.target.clone(), body);
    if reply {
        message.reply_to = event
            .message
            .as_ref()
            .and_then(|item| item.message_id.clone());
    }
    Ok(message)
}

fn incoming_reply_to(invocation: Option<&BotNodeInvocation>) -> Option<String> {
    let payload = invocation?.input.payload.value.clone();
    serde_json::from_value::<BotEvent>(payload)
        .ok()
        .and_then(|event| event.message.and_then(|item| item.message_id))
}

pub fn flow_envelope(
    event: mutsuki_bot_protocol::BotEvent,
    trace_id: Option<String>,
    correlation_id: Option<String>,
) -> Result<BotFlowEventEnvelope, RuntimeError> {
    let context = BotFlowContext {
        bot: Some(event.bot.clone()),
        target: Some(event.target.clone()),
        actor: event.actor.clone(),
        ext: event.ext.clone(),
    };
    Ok(BotFlowEventEnvelope {
        event_id: event.event_id.clone(),
        protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
        payload: BotFlowPayload {
            event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
            value: serde_json::to_value(event)
                .map_err(|error| failure("mutsuki.bot.qqbot.flow.event", error))?,
        },
        context,
        trace_id,
        correlation_id,
    })
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
            format!("runner:{QQBOT_GATEWAY_RUNNER_ID}").into(),
            format!("task_protocol:{QQBOT_GATEWAY_FRAME_PROTOCOL_ID}").into(),
        ],
    }
}

pub fn openapi_descriptor(plugin_generation: u64, media_enabled: bool) -> RunnerDescriptor {
    let mut accepted_protocol_ids = vec![
        BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
        BOT_MESSAGE_RECALL_PROTOCOL_ID.into(),
        QQBOT_ACCOUNT_GET_PROTOCOL_ID.into(),
        QQBOT_GATEWAY_STATUS_PROTOCOL_ID.into(),
        QQBOT_CAPABILITY_GET_PROTOCOL_ID.into(),
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
        contract_surfaces: vec![format!("runner:{QQBOT_OPENAPI_RUNNER_ID}").into()],
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
    let classification = error.classification();
    let retryable = error.retryable();
    let retry_after_ms = error.retry_after_ms();
    let http_status = error.http_status();
    let mut runtime_error = RuntimeError::new(error.stable_code(), QQBOT_ADAPTER_PLUGIN_ID, route);
    runtime_error.recovery = Some(
        match classification {
            crate::api::QqOpenApiErrorClass::RateLimited
            | crate::api::QqOpenApiErrorClass::Transient => "retry",
            crate::api::QqOpenApiErrorClass::Authentication => "refresh_credentials",
            _ => "fix_request",
        }
        .into(),
    );
    runtime_error.evidence = BTreeMap::from([
        (
            "message".into(),
            ScalarValue::String(error.redacted_message()),
        ),
        (
            "classification".into(),
            ScalarValue::String(classification.as_str().into()),
        ),
        ("retryable".into(), ScalarValue::Bool(retryable)),
    ]);
    if let Some(retry_after_ms) = retry_after_ms {
        runtime_error.evidence.insert(
            "retry_after_ms".into(),
            ScalarValue::Int(i64::try_from(retry_after_ms).unwrap_or(i64::MAX)),
        );
    }
    if let Some(http_status) = http_status {
        runtime_error.evidence.insert(
            "http_status".into(),
            ScalarValue::Int(i64::from(http_status)),
        );
    }
    if let QqOpenApiError::HttpStatus { body, .. } = error {
        runtime_error.evidence.insert(
            "body".into(),
            ScalarValue::String(redact_json(&body).to_string()),
        );
    }
    runtime_error
}
