// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::field_reassign_with_default,
    clippy::if_same_then_else,
    clippy::large_futures,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::unused_async
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_agent_client::{AgentClient, AgentClientBackend};
use mutsuki_agent_contracts::{
    AGENT_MESSAGE_CONTEXT_INJECTIONS_META, AgentContentPart, AgentEvent, AgentEventPage,
    AgentMessage, AgentMessageContextInjection, AgentSession, AgentSessionCreateRequest,
    AgentWireError, CONTEXT_SOURCE_ICL, CONTEXT_SOURCE_IDENTIFIERS, CONTEXT_SOURCE_PERSONA,
    SessionSnapshotRef, SessionVersion,
};
use mutsuki_bot_conversation::{
    AgentEventClaim, ConversationError, ConversationService, qq_conversation_from_event,
    session_binding_key,
};
#[cfg(test)]
use mutsuki_bot_protocol::BotNodeWiring;
use mutsuki_bot_protocol::{
    AgentSessionBinding, BOT_AGENT_BIND_PROFILE_PROTOCOL_ID, BOT_AGENT_CANCEL_PROTOCOL_ID,
    BOT_AGENT_FORK_PROTOCOL_ID, BOT_AGENT_REGENERATE_PROTOCOL_ID, BOT_AGENT_RESET_PROTOCOL_ID,
    BOT_AGENT_STATUS_PROTOCOL_ID, BOT_AGENT_SUBMIT_PROTOCOL_ID, BOT_EXT_AGENT_PROFILE_ID,
    BOT_EXT_CONVERSATION_ICL, BOT_EXT_CONVERSATION_IDENTIFIERS, BOT_EXT_PERSONA_PROMPT,
    BOT_EXT_REPLY_SOURCE_MESSAGE_ID, BOT_EXT_REPLY_SOURCE_USER_ID, BOT_FLOW_BOT_EVENT_TYPE,
    BOT_FLOW_DELIVERY_REPLY_TYPE, BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID,
    BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID, BOT_REPLY_DELIVERY_PROTOCOL_ID, BotAgentBridgeRequest,
    BotCommandEvent, BotDeliveryContent, BotEvent, BotFlowContext, BotFlowEventEnvelope,
    BotFlowPayload, BotFlowTypeRef, BotMediaKind, BotMediaSynthesizeRequest,
    BotMediaSynthesizeResult, BotMediaTranscribeRequest, BotMediaTranscribeResult, BotMessage,
    BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodeOutput,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole,
    BotReplyDeliveryCommand, BotReplyDeliveryPart, BotReplyDeliveryReceipt,
    BotReplyDeliveryRequest, BotSpeechReplyPolicy, BotTarget, ConversationIclEntry,
    ConversationIdentifiers, DeliveryPolicy, MessageSegment, QqStreamingStrategy,
    ResolvedConversationPolicy, format_icl_summary,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, InvocationMode, PluginManifest, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerMode, RunnerResult, RunnerSideEffect, Task, TaskOutcome,
    TimeoutGranularity,
};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, BoxedTaskAwaitRunner, PluginBuilder, ProtocolDescriptorBuilder,
    RunnerDescriptorBuilder, RuntimeClientRef, RuntimeFailure, RuntimeResult,
    TaskAwaitRunnerAdapter,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

mod config;

pub use config::{
    BOT_AGENT_CONFIG_PROVIDER_ID, BOT_AGENT_CONFIG_SERVICE_ID, BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
    BOT_AGENT_MIN_MESSAGE_BYTES, BotAgentConfig, BotAgentConfigError, BotAgentConfigHandle,
    BotAgentConfigSnapshot, bot_agent_config_schema,
};

pub const BOT_AGENT_BRIDGE_PLUGIN_ID: &str = "mutsuki.plugin.bot.agent";
pub const BOT_AGENT_BRIDGE_RUNNER_ID: &str = "mutsuki.bot.agent.bridge";
pub const BOT_AGENT_NODE_SUBMIT: &str = "mutsuki.bot.agent.submit";
pub const BOT_AGENT_NODE_CANCEL: &str = "mutsuki.bot.agent.cancel";
pub const BOT_AGENT_NODE_RESET: &str = "mutsuki.bot.agent.reset";
pub const BOT_AGENT_NODE_FORK: &str = "mutsuki.bot.agent.fork";
pub const BOT_AGENT_NODE_STATUS: &str = "mutsuki.bot.agent.status";
pub const BOT_AGENT_NODE_REGENERATE: &str = "mutsuki.bot.agent.regenerate";
pub const BOT_AGENT_NODE_BIND_PROFILE: &str = "mutsuki.bot.agent.bind_profile";

#[must_use]
pub fn bot_agent_bridge_manifest() -> PluginManifest {
    let mut builder =
        PluginBuilder::new(BOT_AGENT_BRIDGE_PLUGIN_ID).runner_descriptor(agent_bridge_descriptor());
    for (protocol_id, binding_name) in [
        (BOT_AGENT_SUBMIT_PROTOCOL_ID, "bot-agent-submit"),
        (BOT_AGENT_CANCEL_PROTOCOL_ID, "bot-agent-cancel"),
        (BOT_AGENT_RESET_PROTOCOL_ID, "bot-agent-reset"),
        (BOT_AGENT_FORK_PROTOCOL_ID, "bot-agent-fork"),
        (BOT_AGENT_STATUS_PROTOCOL_ID, "bot-agent-status"),
        (BOT_AGENT_REGENERATE_PROTOCOL_ID, "bot-agent-regenerate"),
        (BOT_AGENT_BIND_PROFILE_PROTOCOL_ID, "bot-agent-bind-profile"),
    ] {
        builder = builder.protocol_handler(
            ProtocolDescriptorBuilder::new(protocol_id)
                .input_schema(serde_json::json!({
                    "type": "object",
                    "required": ["action", "event"]
                }))
                .output_schema(serde_json::json!({
                    "type": "object",
                    "required": ["binding", "turn_id", "outgoing", "resolved"]
                }))
                .error_schema(serde_json::json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            BOT_AGENT_BRIDGE_RUNNER_ID,
            binding_name,
        );
    }
    builder
        .extension(
            agent_node_catalog()
                .into_plugin_extension()
                .expect("Agent node catalog serializes"),
        )
        .build()
        .manifest
}

fn agent_node_catalog() -> BotNodeCatalogFragment {
    let definitions = [
        (
            BOT_AGENT_NODE_SUBMIT,
            "提交 Agent",
            BOT_AGENT_SUBMIT_PROTOCOL_ID,
            BOT_FLOW_BOT_EVENT_TYPE,
        ),
        (
            BOT_AGENT_NODE_CANCEL,
            "取消回复",
            BOT_AGENT_CANCEL_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_RESET,
            "重置会话",
            BOT_AGENT_RESET_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_FORK,
            "分叉会话",
            BOT_AGENT_FORK_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_STATUS,
            "会话状态",
            BOT_AGENT_STATUS_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
        (
            BOT_AGENT_NODE_REGENERATE,
            "重新生成",
            BOT_AGENT_REGENERATE_PROTOCOL_ID,
            "mutsuki.bot.command.event",
        ),
    ];
    let mut nodes = definitions
        .into_iter()
        .map(
            |(node_type_id, title, protocol_id, input_type)| BotNodeDescriptor {
                node_type_id: node_type_id.into(),
                version: 1,
                title: title.into(),
                category: "Agent".into(),
                role: BotNodeRole::Processor,
                binding: Some(BotNodeBinding {
                    binding_id: format!("binding:{protocol_id}"),
                    protocol_id: protocol_id.into(),
                    runner_hint: Some(BOT_AGENT_BRIDGE_RUNNER_ID.into()),
                }),
                ports: vec![
                    BotNodePortDescriptor {
                        port_id: "input".into(),
                        title: "输入".into(),
                        direction: BotNodePortDirection::Input,
                        event_type: BotFlowTypeRef::new(input_type, 1),
                        required: true,
                    },
                    BotNodePortDescriptor {
                        port_id: "reply".into(),
                        title: "回复".into(),
                        direction: BotNodePortDirection::Output,
                        event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                        required: false,
                    },
                ],
                config_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            },
        )
        .collect::<Vec<_>>();
    nodes.push(BotNodeDescriptor {
        node_type_id: BOT_AGENT_NODE_BIND_PROFILE.into(),
        version: 1,
        title: "绑定人格".into(),
        category: "Agent".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{BOT_AGENT_BIND_PROFILE_PROTOCOL_ID}"),
            protocol_id: BOT_AGENT_BIND_PROFILE_PROTOCOL_ID.into(),
            runner_hint: Some(BOT_AGENT_BRIDGE_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "input".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "output".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
        ],
        config_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "profile_id": {"type": "string", "minLength": 1, "title": "Agent 配置文件"},
                "override": {"type": "boolean", "default": false, "title": "覆盖已有人格"}
            }
        }),
    });
    BotNodeCatalogFragment { nodes }
}

#[must_use]
pub fn agent_bridge_runner(client: RuntimeClientRef, bridge: BotAgentBridge) -> Box<dyn Runner> {
    agent_bridge_runner_with_delivery_policy(client, bridge, default_reply_delivery_policy())
}

#[must_use]
pub fn agent_bridge_runner_with_delivery_policy(
    client: RuntimeClientRef,
    bridge: BotAgentBridge,
    delivery_policy: DeliveryPolicy,
) -> Box<dyn Runner> {
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let bridge = bridge.clone();
        let delivery_policy = delivery_policy.clone();
        let config = bridge.config.snapshot();
        let permit = bridge.concurrency.try_acquire(config.max_concurrency);
        Box::pin(async move {
            let Some(_permit) = permit else {
                return Err(bridge_failure(
                    &task,
                    "concurrency_limited",
                    format!(
                        "Bot Agent concurrency limit {} is currently occupied",
                        config.max_concurrency
                    ),
                ));
            };
            run_bridge_task(ctx, task, bridge, delivery_policy).await
        })
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(agent_bridge_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

fn default_reply_delivery_policy() -> DeliveryPolicy {
    DeliveryPolicy {
        max_attempts: 3,
        initial_backoff_ms: 1_000,
        max_backoff_ms: 60_000,
        not_before_unix_ms: None,
        expires_at_unix_ms: None,
    }
}

fn agent_bridge_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_BRIDGE_PLUGIN_ID)
        .accepted_protocol(BOT_AGENT_SUBMIT_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_CANCEL_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_RESET_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_FORK_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_STATUS_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_REGENERATE_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_BIND_PROFILE_PROTOCOL_ID)
        .requires_protocol(BOT_REPLY_DELIVERY_PROTOCOL_ID)
        .requires_protocol(BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID)
        .requires_protocol(BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .invocation_mode(InvocationMode::AsyncReentrant)
        .concurrency(RunnerConcurrency::Reentrant {
            max_inflight_batches: 128,
            max_inflight_entries: 128,
        })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 1,
            max_batch_entries: 1,
            max_entry_concurrency: 1,
            max_inflight_batches: 128,
            side_effect: RunnerSideEffect::External,
            ..RunnerBatchCapability::default()
        })
        .control_capability(RunnerControlCapability {
            entry_cancel: true,
            batch_cancel: true,
            timeout_granularity: TimeoutGranularity::Entry,
        })
        .build()
}

async fn run_bridge_task(
    ctx: AsyncRunnerContext,
    task: Task,
    bridge: BotAgentBridge,
    delivery_policy: DeliveryPolicy,
) -> RuntimeResult<RunnerResult> {
    if let Ok(invocation) = task.payload.decode_shared::<BotNodeInvocation>() {
        return run_bridge_node_task(
            ctx,
            task,
            bridge,
            delivery_policy,
            invocation.as_ref().clone(),
        )
        .await;
    }
    let request = decode_bridge_request(&task)?;
    let execution = execute_bridge_request(&ctx, &task, &bridge, request).await?;
    let result = &execution.result;
    let (reply, media_errors) = if let Some(existing) = execution.existing_reply {
        (Some(existing), Vec::new())
    } else {
        let (outgoing, media_errors) = speech_reply_messages(&ctx, &task, result).await;
        if outgoing.is_empty() {
            (None, media_errors)
        } else {
            let request =
                reply_delivery_request(result, &execution.source_event, outgoing, delivery_policy)
                    .map_err(|error| bridge_failure(&task, "delivery.binding", error))?;
            let outcome = ctx
                .call_raw(
                    BOT_REPLY_DELIVERY_PROTOCOL_ID,
                    serde_json::to_value(BotReplyDeliveryCommand::Submit {
                        request: Box::new(request),
                        now_unix_ms: unix_ms(),
                    })
                    .map_err(|error| bridge_failure(&task, "delivery.encode", error))?,
                )
                .await;
            (
                Some(decode_reply_delivery_outcome(&task, outcome)?),
                media_errors,
            )
        }
    };
    if execution.complete_event {
        bridge
            .complete_event(&execution.source_event)
            .await
            .map_err(|error| bridge_failure(&task, "event.complete", error))?;
    }
    let mut completed = RunnerResult::completed(task.task_id);
    completed.output = Some(serde_json::json!({
        "session_id": result.binding.session_id,
        "session_version": result.binding.session_version,
        "last_event_sequence": result.binding.last_event_sequence,
        "turn_id": result.turn_id,
        "reply_delivery": reply,
        "media_errors": media_errors,
    }));
    Ok(completed)
}

async fn run_bridge_node_task(
    ctx: AsyncRunnerContext,
    task: Task,
    bridge: BotAgentBridge,
    delivery_policy: DeliveryPolicy,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    if task.protocol_id == BOT_AGENT_BIND_PROFILE_PROTOCOL_ID {
        return bind_profile_node(&task, invocation);
    }
    let request = flow_bridge_request(&task, &invocation)?;
    let execution = execute_bridge_request(&ctx, &task, &bridge, request).await?;
    if execution.existing_reply.is_some() {
        let mut completed = RunnerResult::completed(task.task_id.clone());
        completed.output = Some(
            serde_json::to_value(BotNodeResult {
                outputs: Vec::new(),
                metadata: std::collections::BTreeMap::from([(
                    "delivery_already_reserved".into(),
                    serde_json::Value::Bool(true),
                )]),
            })
            .map_err(|error| bridge_failure(&task, "node.output", error))?,
        );
        return Ok(completed);
    }
    let (outgoing, media_errors) = speech_reply_messages(&ctx, &task, &execution.result).await;
    let outputs = if outgoing.is_empty() {
        if execution.complete_event {
            bridge
                .complete_event(&execution.source_event)
                .await
                .map_err(|error| bridge_failure(&task, "event.complete", error))?;
        }
        Vec::new()
    } else {
        let mut request = reply_delivery_request(
            &execution.result,
            &execution.source_event,
            outgoing,
            delivery_policy,
        )
        .map_err(|error| bridge_failure(&task, "delivery.binding", error))?;
        request.occupancy_only = true;
        reserve_reply_delivery(&ctx, &task, &request).await?;
        vec![BotNodeOutput {
            port_id: "reply".into(),
            event: BotFlowEventEnvelope {
                event_id: request.reply_id.clone(),
                protocol_id: BOT_REPLY_DELIVERY_PROTOCOL_ID.into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                    value: serde_json::to_value(&request)
                        .map_err(|error| bridge_failure(&task, "delivery.encode", error))?,
                },
                context: overlay_reply_source_context(
                    invocation.input.context.clone(),
                    &execution.source_event,
                ),
                trace_id: invocation.input.trace_id.clone(),
                correlation_id: invocation.input.correlation_id.clone(),
            },
        }]
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert(
        "session_id".into(),
        serde_json::Value::String(execution.result.binding.session_id.clone()),
    );
    metadata.insert(
        "turn_id".into(),
        serde_json::Value::String(execution.result.turn_id.clone()),
    );
    metadata.insert(
        "media_errors".into(),
        serde_json::to_value(media_errors)
            .map_err(|error| bridge_failure(&task, "node.metadata", error))?,
    );
    let mut completed = RunnerResult::completed(task.task_id.clone());
    completed.output = Some(
        serde_json::to_value(BotNodeResult { outputs, metadata })
            .map_err(|error| bridge_failure(&task, "node.output", error))?,
    );
    Ok(completed)
}

#[allow(clippy::needless_pass_by_value)]
fn bind_profile_node(task: &Task, invocation: BotNodeInvocation) -> RuntimeResult<RunnerResult> {
    let mut event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
        .map_err(|error| bridge_failure(task, "node.event", error))?;
    let override_existing = invocation
        .config
        .get("override")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let has_existing_profile = event
        .ext
        .get(BOT_EXT_AGENT_PROFILE_ID)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if let Some(profile_id) = invocation
        .config
        .get("profile_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && (!has_existing_profile || override_existing)
    {
        event.ext.insert(
            BOT_EXT_AGENT_PROFILE_ID.into(),
            serde_json::Value::String(profile_id.to_owned()),
        );
    }
    let mut output = invocation.input.clone();
    output.payload.value =
        serde_json::to_value(&event).map_err(|error| bridge_failure(task, "node.encode", error))?;
    let mut completed = RunnerResult::completed(task.task_id.clone());
    completed.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![BotNodeOutput {
                port_id: "output".into(),
                event: output,
            }],
            metadata: BTreeMap::default(),
        })
        .map_err(|error| bridge_failure(task, "node.output", error))?,
    );
    Ok(completed)
}

fn flow_bridge_request(
    task: &Task,
    invocation: &BotNodeInvocation,
) -> RuntimeResult<BotAgentBridgeRequest> {
    if task.protocol_id == BOT_AGENT_SUBMIT_PROTOCOL_ID {
        let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
            .map_err(|error| bridge_failure(task, "node.event", error))?;
        return Ok(BotAgentBridgeRequest::Submit { event });
    }
    let command: BotCommandEvent = serde_json::from_value(invocation.input.payload.value.clone())
        .map_err(|error| bridge_failure(task, "node.command", error))?;
    let event = command.source;
    match task.protocol_id.as_str() {
        BOT_AGENT_CANCEL_PROTOCOL_ID => {
            let turn_id = command
                .typed_args
                .get("turn_id")
                .and_then(|value| match value {
                    mutsuki_bot_protocol::BotCommandArgumentValue::String(value) => {
                        Some(value.clone())
                    }
                    _ => None,
                })
                .or_else(|| command.args.first().cloned())
                .ok_or_else(|| bridge_failure(task, "node.cancel", "turn_id is required"))?;
            Ok(BotAgentBridgeRequest::Cancel { event, turn_id })
        }
        BOT_AGENT_RESET_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Reset { event }),
        BOT_AGENT_FORK_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Fork { event }),
        BOT_AGENT_STATUS_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Status { event }),
        BOT_AGENT_REGENERATE_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Regenerate { event }),
        protocol => Err(bridge_failure(task, "node.protocol", protocol)),
    }
}

struct BridgeExecution {
    result: BotAgentBridgeResult,
    source_event: BotEvent,
    complete_event: bool,
    existing_reply: Option<BotReplyDeliveryReceipt>,
}

fn decode_bridge_request(task: &Task) -> RuntimeResult<BotAgentBridgeRequest> {
    let payload = task.payload.to_value();
    if let Ok(request) = serde_json::from_value::<BotAgentBridgeRequest>(payload.clone()) {
        return Ok(request);
    }
    if let Ok(command) = serde_json::from_value::<BotCommandEvent>(payload.clone()) {
        return bridge_request_from_command(command)
            .map_err(|error| bridge_failure(task, "command.decode", error));
    }
    serde_json::from_value::<BotEvent>(payload)
        .map(|event| BotAgentBridgeRequest::Submit { event })
        .map_err(|error| bridge_failure(task, "request.decode", error))
}

async fn execute_bridge_request(
    ctx: &AsyncRunnerContext,
    task: &Task,
    bridge: &BotAgentBridge,
    request: BotAgentBridgeRequest,
) -> RuntimeResult<BridgeExecution> {
    match request {
        BotAgentBridgeRequest::Submit { event } => {
            submit_claimed_event(ctx, task, bridge, event).await
        }
        BotAgentBridgeRequest::Regenerate { mut event } => {
            let binding = bridge
                .status(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            event.event_id = format!("{}:regenerate:{}", event.event_id, binding.generation);
            submit_claimed_event(ctx, task, bridge, event).await
        }
        BotAgentBridgeRequest::Cancel { event, turn_id } => {
            let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .status(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .cancel(&resolved, &binding, actor_id, &turn_id)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: command_confirmation(&event.target, "已取消当前 Agent 回复"),
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
        BotAgentBridgeRequest::Reset { event } => {
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
            let binding = bridge
                .conversations
                .reset_session_binding(&resolved, actor_id)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing: command_confirmation(&event.target, "已开启新的 Agent 会话"),
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
        BotAgentBridgeRequest::Fork { event } => {
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .fork(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing: command_confirmation(&event.target, "已分叉 Agent 会话，历史记录已保留"),
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
        BotAgentBridgeRequest::Status { event } => {
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .status(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let outgoing = status_confirmation(&event.target, &binding);
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing,
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
    }
}

async fn submit_claimed_event(
    ctx: &AsyncRunnerContext,
    task: &Task,
    bridge: &BotAgentBridge,
    event: BotEvent,
) -> RuntimeResult<BridgeExecution> {
    let (claim, claimed) = bridge
        .claim_event_state(&event)
        .await
        .map_err(|error| bridge_failure(task, "claim", error))?;
    if claim != AgentEventClaim::New {
        let reply_id = stable_reply_id(
            &claimed.resolved.conversation.origin_key(),
            &event.event_id,
            &claimed.turn_id,
        );
        match inspect_reply_delivery(ctx, task, reply_id).await? {
            Some(receipt) => {
                return Ok(BridgeExecution {
                    result: claimed,
                    source_event: event,
                    complete_event: claim == AgentEventClaim::ResumePending,
                    existing_reply: Some(receipt),
                });
            }
            None if claim == AgentEventClaim::Completed => {
                return Ok(BridgeExecution {
                    result: claimed,
                    source_event: event,
                    complete_event: false,
                    existing_reply: None,
                });
            }
            None => {}
        }
    }
    let event = transcribe_event_audio(ctx, task, bridge, event).await?;
    let result = bridge
        .submit_event_with_trace_deferred(&event, trace_context(task).as_ref())
        .await
        .map_err(|error| bridge_failure(task, "action", error))?;
    Ok(BridgeExecution {
        result,
        source_event: event,
        complete_event: true,
        existing_reply: None,
    })
}

async fn inspect_reply_delivery(
    ctx: &AsyncRunnerContext,
    task: &Task,
    reply_id: String,
) -> RuntimeResult<Option<BotReplyDeliveryReceipt>> {
    let outcome = ctx
        .call_raw(
            BOT_REPLY_DELIVERY_PROTOCOL_ID,
            serde_json::to_value(BotReplyDeliveryCommand::Inspect { reply_id })
                .map_err(|error| bridge_failure(task, "delivery.inspect.encode", error))?,
        )
        .await
        .map(|value| value.into_outcome());
    match outcome {
        Ok(TaskOutcome::Completed {
            output: Some(output),
            ..
        }) => serde_json::from_value(output)
            .map(Some)
            .map_err(|error| bridge_failure(task, "delivery.inspect.decode", error)),
        Ok(TaskOutcome::Failed { error, .. }) if error.code == "delivery.not_found" => Ok(None),
        Ok(TaskOutcome::Failed { error, .. }) => {
            Err(bridge_failure(task, "delivery.inspect", error.code))
        }
        Ok(outcome) => Err(bridge_failure(
            task,
            "delivery.inspect",
            format!("unexpected delivery outcome {outcome:?}"),
        )),
        Err(error) => Err(error),
    }
}

async fn reserve_reply_delivery(
    ctx: &AsyncRunnerContext,
    task: &Task,
    request: &BotReplyDeliveryRequest,
) -> RuntimeResult<BotReplyDeliveryReceipt> {
    let outcome = ctx
        .call_raw(
            BOT_REPLY_DELIVERY_PROTOCOL_ID,
            serde_json::to_value(BotReplyDeliveryCommand::Reserve {
                request: Box::new(request.clone()),
            })
            .map_err(|error| bridge_failure(task, "delivery.reserve.encode", error))?,
        )
        .await;
    decode_reply_delivery_outcome(task, outcome)
}

fn overlay_reply_source_context(mut context: BotFlowContext, event: &BotEvent) -> BotFlowContext {
    if let Some(message_id) = event
        .message
        .as_ref()
        .and_then(|message| message.message_id.clone())
    {
        context.ext.insert(
            BOT_EXT_REPLY_SOURCE_MESSAGE_ID.into(),
            serde_json::Value::String(message_id),
        );
    }
    if let Some(user_id) = event.actor.as_ref().map(|actor| actor.user_id.clone()) {
        context.ext.insert(
            BOT_EXT_REPLY_SOURCE_USER_ID.into(),
            serde_json::Value::String(user_id),
        );
    }
    context
}

fn decode_reply_delivery_outcome(
    task: &Task,
    outcome: RuntimeResult<impl Into<TaskOutcome>>,
) -> RuntimeResult<BotReplyDeliveryReceipt> {
    match outcome?.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output)
            .map_err(|error| bridge_failure(task, "delivery.decode", error)),
        TaskOutcome::Failed { error, .. } => {
            Err(bridge_failure(task, "delivery.submit", error.code))
        }
        outcome => Err(bridge_failure(
            task,
            "delivery.submit",
            format!("unexpected delivery outcome {outcome:?}"),
        )),
    }
}

fn reply_delivery_request(
    result: &BotAgentBridgeResult,
    event: &BotEvent,
    outgoing: Vec<BotMessage>,
    policy: DeliveryPolicy,
) -> Result<BotReplyDeliveryRequest, ConversationError> {
    let turn_id = if result.turn_id.trim().is_empty() {
        format!("bot-action:{}", event.event_id)
    } else {
        result.turn_id.clone()
    };
    let reply_id = stable_reply_id(
        &result.resolved.conversation.origin_key(),
        &event.event_id,
        &turn_id,
    );
    let parts = outgoing
        .into_iter()
        .enumerate()
        .map(|(index, message)| BotReplyDeliveryPart {
            part_id: format!("{reply_id}:part:{index}"),
            content: BotDeliveryContent {
                segments: message.segments,
                summary: None,
                reply_to: message.reply_to,
            },
            not_before_unix_ms: None,
        })
        .collect();
    let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
    let source_binding_key = session_binding_key(
        &result.resolved.conversation,
        result.resolved.policy.session_scope,
        actor_id,
    )?;
    Ok(BotReplyDeliveryRequest {
        idempotency_key: reply_id.clone(),
        reply_id,
        conversation: result.resolved.conversation.clone(),
        parts,
        policy,
        source_event_id: event.event_id.clone(),
        source_turn_id: turn_id,
        source_binding_key: Some(source_binding_key),
        occupancy_only: false,
    })
}

fn stable_reply_id(origin_key: &str, event_id: &str, turn_id: &str) -> String {
    let mut digest = Sha256::new();
    for value in [origin_key, event_id, turn_id] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    let mut id = String::from("agent-reply:");
    for byte in digest.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to String cannot fail");
    }
    id
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn bridge_request_from_command(
    command: BotCommandEvent,
) -> Result<BotAgentBridgeRequest, BotAgentError> {
    let mut event = command.source;
    match command.name.as_str() {
        "ask" | "chat" => {
            let prompt = command.args.join(" ");
            if prompt.trim().is_empty() {
                return Err(BotAgentError::InvalidCommand);
            }
            event.message = Some(BotMessage::text(event.target.clone(), prompt));
            Ok(BotAgentBridgeRequest::Submit { event })
        }
        "cancel" => command
            .args
            .first()
            .filter(|turn_id| !turn_id.trim().is_empty())
            .cloned()
            .map(|turn_id| BotAgentBridgeRequest::Cancel { event, turn_id })
            .ok_or(BotAgentError::InvalidCommand),
        "reset" => Ok(BotAgentBridgeRequest::Reset { event }),
        "fork" => Ok(BotAgentBridgeRequest::Fork { event }),
        "status" => Ok(BotAgentBridgeRequest::Status { event }),
        "regenerate" => Ok(BotAgentBridgeRequest::Regenerate { event }),
        _ => Err(BotAgentError::InvalidCommand),
    }
}

fn command_confirmation(target: &BotTarget, text: &str) -> Vec<BotMessage> {
    vec![BotMessage::text(target.clone(), text)]
}

fn status_confirmation(target: &BotTarget, binding: &AgentSessionBinding) -> Vec<BotMessage> {
    let text = if binding.session_version == 0 {
        "当前 Agent 会话尚未开始".to_owned()
    } else {
        format!("当前 Agent 会话已完成 {} 轮对话", binding.session_version)
    };
    command_confirmation(target, &text)
}

async fn transcribe_event_audio(
    ctx: &AsyncRunnerContext,
    task: &Task,
    bridge: &BotAgentBridge,
    mut event: BotEvent,
) -> RuntimeResult<BotEvent> {
    let resolved = bridge
        .resolve_admitted(&event)
        .await
        .map_err(|error| bridge_failure(task, "policy", error))?;
    if !resolved.policy.stt_enabled {
        return Ok(event);
    }
    let audio = event
        .message
        .as_ref()
        .into_iter()
        .flat_map(|message| &message.segments)
        .filter_map(|segment| match segment {
            MessageSegment::Audio { resource } => Some(resource.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut transcripts = Vec::with_capacity(audio.len());
    for resource in audio {
        let outcome = ctx
            .call_raw(
                BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID,
                serde_json::to_value(BotMediaTranscribeRequest {
                    audio: resource,
                    language: None,
                    prompt_hint: None,
                    selector_id: resolved.policy.stt_selector_id.clone(),
                    timeout_ms: None,
                })
                .map_err(|error| bridge_failure(task, "stt.encode", error))?,
            )
            .await?;
        transcripts
            .push(decode_child_output::<BotMediaTranscribeResult>(outcome, task, "stt")?.text);
    }
    if let (Some(message), false) = (event.message.as_mut(), transcripts.is_empty()) {
        let transcript = transcripts.join("\n");
        let transcript = if message.plain_text().is_empty() {
            transcript
        } else {
            format!("\n{transcript}")
        };
        message.segments.push(MessageSegment::text(transcript));
    }
    Ok(event)
}

async fn speech_reply_messages(
    ctx: &AsyncRunnerContext,
    task: &Task,
    result: &BotAgentBridgeResult,
) -> (Vec<BotMessage>, Vec<String>) {
    if !result.resolved.policy.tts_enabled {
        return (result.outgoing.clone(), Vec::new());
    }
    let policy = result.resolved.policy.speech_reply_policy;
    let mut outgoing = Vec::new();
    let mut errors = Vec::new();
    for message in &result.outgoing {
        let text = message.plain_text();
        if text.is_empty() {
            outgoing.push(message.clone());
            continue;
        }
        let synthesis = ctx
            .call_raw(
                BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID,
                match serde_json::to_value(BotMediaSynthesizeRequest {
                    target: message.target.clone(),
                    text: text.clone(),
                    policy,
                    voice_selector_id: result.resolved.policy.tts_selector_id.clone(),
                    timeout_ms: None,
                    qq_mime_type: "audio/silk".into(),
                }) {
                    Ok(value) => value,
                    Err(error) => {
                        errors.push(error.to_string());
                        if policy != BotSpeechReplyPolicy::VoiceOnly {
                            outgoing.push(message.clone());
                        }
                        continue;
                    }
                },
            )
            .await;
        let synthesized = synthesis.and_then(|outcome| {
            decode_child_output::<BotMediaSynthesizeResult>(outcome, task, "tts")
        });
        match synthesized {
            Ok(synthesized) => {
                if let Some(text) = synthesized.text {
                    outgoing.push(BotMessage::text(message.target.clone(), text));
                }
                if let Some(upload) = synthesized.upload {
                    let segment = match upload.kind {
                        BotMediaKind::Image => MessageSegment::Image {
                            resource: upload.resource,
                        },
                        BotMediaKind::Audio => MessageSegment::Audio {
                            resource: upload.resource,
                        },
                        BotMediaKind::Video => MessageSegment::Video {
                            resource: upload.resource,
                        },
                        BotMediaKind::File => MessageSegment::File {
                            resource: upload.resource,
                            name: upload.file_name,
                        },
                    };
                    outgoing.push(BotMessage {
                        message_id: None,
                        target: message.target.clone(),
                        sender: None,
                        segments: vec![segment],
                        reply_to: message.reply_to.clone(),
                        time_ms: None,
                        ext: std::collections::BTreeMap::default(),
                    });
                }
            }
            Err(error) => {
                errors.push(error.error().code.clone());
                if policy != BotSpeechReplyPolicy::VoiceOnly {
                    outgoing.push(message.clone());
                }
            }
        }
    }
    (outgoing, errors)
}

fn decode_child_output<T: serde::de::DeserializeOwned>(
    outcome: impl Into<TaskOutcome>,
    task: &Task,
    route: &str,
) -> RuntimeResult<T> {
    match outcome.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output)
            .map_err(|error| bridge_failure(task, &format!("{route}.output"), error)),
        TaskOutcome::Failed { error, .. } => Err(RuntimeFailure::new(error)),
        TaskOutcome::Completed { output: None, .. }
        | TaskOutcome::Cancelled { .. }
        | TaskOutcome::Expired { .. }
        | TaskOutcome::DeadLetter { .. } => {
            Err(bridge_failure(task, route, "child did not complete"))
        }
    }
}

fn bridge_failure(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeFailure {
    let mut runtime_error = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_AGENT_BRIDGE_PLUGIN_ID,
        format!("{}.{}", task.task_id, route),
    );
    runtime_error.evidence.insert(
        "reason".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    RuntimeFailure::new(runtime_error)
}

pub trait AgentBridgeClient: Send {
    /// Loads one Agent session snapshot.
    ///
    /// # Errors
    ///
    /// Returns the Agent wire error reported by the session owner.
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError>;
    /// Creates one Agent session.
    ///
    /// # Errors
    ///
    /// Returns the Agent wire error reported by the session owner.
    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError>;
    /// Submits an idempotent Agent turn against the expected session version.
    ///
    /// # Errors
    ///
    /// Returns a typed wire error for stale versions, invalid input, or execution failure.
    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError>;
    /// Cancels an Agent turn with optimistic session versioning.
    ///
    /// # Errors
    ///
    /// Returns the typed Agent wire failure.
    fn cancel_turn(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError>;
    /// Forks an Agent session into an owner-provided target identity.
    ///
    /// # Errors
    ///
    /// Returns the typed Agent wire failure.
    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError>;
    /// Reads committed Agent events after the supplied sequence.
    ///
    /// # Errors
    ///
    /// Returns the typed Agent wire failure.
    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError>;
}

impl<B: AgentClientBackend + Send> AgentBridgeClient for AgentClient<B> {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.get_session(session_id)
    }

    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        self.start_session(request)
    }

    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        self.submit_turn(
            session_id,
            expected_version,
            turn_id,
            messages,
            idempotency_key,
        )
    }

    fn cancel_turn(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        self.cancel_turn(session_id, turn_id, expected_version)
    }

    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let session = self.get_session(source_session_id)?;
        self.fork_session(
            source_session_id,
            target_session_id,
            SessionSnapshotRef {
                session_id: source_session_id.into(),
                version: expected_version,
                snapshot: session.resource,
                base: None,
                deltas: Vec::new(),
            },
        )
    }

    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
        self.resume_session_events(session_id, after_sequence)
    }
}

#[derive(Clone)]
pub struct BotAgentBridge {
    conversations: ConversationService,
    client: Arc<Mutex<Box<dyn AgentBridgeClient>>>,
    config: BotAgentConfigHandle,
    concurrency: Arc<BotAgentConcurrencyGate>,
}

#[derive(Default)]
struct BotAgentConcurrencyGate {
    active: Mutex<usize>,
}

struct BotAgentConcurrencyPermit {
    gate: Arc<BotAgentConcurrencyGate>,
}

impl BotAgentConcurrencyGate {
    fn try_acquire(self: &Arc<Self>, limit: usize) -> Option<BotAgentConcurrencyPermit> {
        let mut active = self.active.lock().expect("Bot Agent gate mutex");
        if *active >= limit.max(1) {
            return None;
        }
        *active += 1;
        Some(BotAgentConcurrencyPermit { gate: self.clone() })
    }
}

impl Drop for BotAgentConcurrencyPermit {
    fn drop(&mut self) {
        let mut active = self.gate.active.lock().expect("Bot Agent gate mutex");
        *active = active.saturating_sub(1);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotAgentTraceContext {
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
}

impl BotAgentBridge {
    #[must_use]
    /// Creates a bridge with a validated default policy and selected streaming strategy.
    ///
    /// # Panics
    ///
    /// Panics only if an internal enum-to-config mapping produces an invalid default snapshot.
    pub fn new(
        conversations: ConversationService,
        client: Box<dyn AgentBridgeClient>,
        streaming: &QqStreamingStrategy,
    ) -> Self {
        let config = BotAgentConfigHandle::default();
        let mut settings = config.snapshot();
        settings.enabled = true;
        settings.connection_id = "injected".into();
        settings.streaming = streaming_name(streaming).into();
        config
            .replace(settings)
            .expect("streaming strategy must produce a valid Bot Agent config");
        Self::new_with_config(conversations, client, config)
    }

    #[must_use]
    pub fn new_with_config(
        conversations: ConversationService,
        client: Box<dyn AgentBridgeClient>,
        config: BotAgentConfigHandle,
    ) -> Self {
        Self {
            conversations,
            client: Arc::new(Mutex::new(client)),
            config,
            concurrency: Arc::new(BotAgentConcurrencyGate::default()),
        }
    }

    /// Resolves, admits, and submits one Bot event to the bound Agent session.
    ///
    /// # Errors
    ///
    /// Returns a typed policy, session, media, or Agent execution error.
    pub async fn submit_event(
        &self,
        event: &BotEvent,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        self.submit_event_with_trace(event, None).await
    }

    /// Submits one Bot event while preserving the supplied trace context.
    ///
    /// # Errors
    ///
    /// Returns a typed policy, session, media, or Agent execution error.
    pub async fn submit_event_with_trace(
        &self,
        event: &BotEvent,
        trace: Option<&BotAgentTraceContext>,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        let result = self.submit_event_with_trace_deferred(event, trace).await?;
        self.complete_event(event).await?;
        Ok(result)
    }

    async fn submit_event_with_trace_deferred(
        &self,
        event: &BotEvent,
        trace: Option<&BotAgentTraceContext>,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let (resolved, config) = self.resolve_admitted_with_config(event).await?;
        let profile_id = resolved
            .policy
            .agent_runtime_profile_id
            .clone()
            .ok_or(BotAgentError::AgentProfileMissing)?;
        let message = event_message(event, trace)?;
        let turn_id = format!("qq:{}", event.event_id);
        let binding = self
            .conversations
            .get_or_create_session_binding(&resolved, actor_id)
            .await?;
        if self
            .conversations
            .begin_agent_event(&resolved, actor_id, &event.event_id, &turn_id)
            .await?
            == AgentEventClaim::Completed
        {
            return Ok(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: Vec::new(),
            });
        }
        let next_version = {
            let mut client = self
                .client
                .lock()
                .map_err(|_| BotAgentError::ClientPoisoned)?;
            match client.get_session(&binding.session_id) {
                Ok(_) => {}
                Err(error) if error.code == "agent.session.not_found" => {
                    client.start_session(AgentSessionCreateRequest {
                        session_id: Some(binding.session_id.clone()),
                        profile_id,
                        title: Some(format!("QQ {}", resolved.conversation.origin_key())),
                    })?;
                }
                Err(error) => return Err(error.into()),
            }
            client.submit_turn(
                &binding.session_id,
                SessionVersion(binding.session_version),
                &turn_id,
                vec![message],
                &event.event_id,
            )?
        };
        let binding = self
            .advance_or_reuse_session_version(&resolved, actor_id, &binding, next_version.0)
            .await?;
        let page = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .events(&binding.session_id, binding.last_event_sequence)?;
        let strategy = config.streaming_strategy()?;
        let outgoing =
            outgoing_messages_with_limit(event, &page, &strategy, config.max_message_bytes);
        let binding = self
            .conversations
            .advance_event_sequence(
                &resolved,
                actor_id,
                binding.last_event_sequence,
                page.next_sequence,
            )
            .await?;
        Ok(BotAgentBridgeResult {
            resolved,
            binding,
            turn_id,
            outgoing,
        })
    }

    /// Claims a QQ event before STT/TTS or any other externally metered media work.
    ///
    /// A pending claim remains resumable after a Host restart; only a completed claim bypasses
    /// the rest of the bridge.
    /// Claims an event before any media side effect and detects completed retries.
    ///
    /// # Errors
    ///
    /// Returns an error when conversation resolution or the durable claim fails.
    pub async fn claim_event_before_media(
        &self,
        event: &BotEvent,
    ) -> Result<Option<BotAgentBridgeResult>, BotAgentError> {
        let (claim, result) = self.claim_event_state(event).await?;
        Ok((claim == AgentEventClaim::Completed).then_some(result))
    }

    async fn claim_event_state(
        &self,
        event: &BotEvent,
    ) -> Result<(AgentEventClaim, BotAgentBridgeResult), BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let (resolved, _) = self.resolve_admitted_with_config(event).await?;
        let binding = self
            .conversations
            .get_or_create_session_binding(&resolved, actor_id)
            .await?;
        let turn_id = format!("qq:{}", event.event_id);
        let claim = self
            .conversations
            .begin_agent_event(&resolved, actor_id, &event.event_id, &turn_id)
            .await?;
        Ok((
            claim,
            BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: Vec::new(),
            },
        ))
    }

    async fn complete_event(&self, event: &BotEvent) -> Result<(), BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        self.conversations
            .complete_agent_event(&resolved, actor_id, &event.event_id)
            .await?;
        Ok(())
    }

    /// Resolves the effective conversation policy for an event.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported targets, invalid identity, or repository failure.
    pub async fn resolve(
        &self,
        event: &BotEvent,
    ) -> Result<ResolvedConversationPolicy, BotAgentError> {
        Ok(self
            .conversations
            .resolve_execution(qq_conversation_from_event(event)?)?)
    }

    async fn advance_or_reuse_session_version(
        &self,
        resolved: &ResolvedConversationPolicy,
        actor_id: Option<&str>,
        binding: &AgentSessionBinding,
        next_session_version: u64,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        if next_session_version != binding.session_version {
            return Ok(self
                .conversations
                .advance_session(
                    resolved,
                    actor_id,
                    binding.session_version,
                    next_session_version,
                )
                .await?);
        }

        // AgentClient may return the already-accepted version when a reconnect retries the
        // same idempotency key after the first submit succeeded but event retrieval disconnected.
        // Reusing the persisted binding is safe only while reset/fork/concurrent progress has not
        // replaced this session lineage.
        let current = self
            .conversations
            .session_binding(resolved, actor_id)
            .await?
            .ok_or(ConversationError::BindingNotFound)?;
        if current.session_id != binding.session_id {
            return Err(ConversationError::GenerationConflict.into());
        }
        if current.session_version != next_session_version {
            return Err(ConversationError::SessionVersionConflict {
                expected: next_session_version,
                actual: current.session_version,
            }
            .into());
        }
        Ok(current)
    }

    /// Resolves policy and enforces admission without submitting an Agent turn.
    ///
    /// # Errors
    ///
    /// Returns the stable policy or admission error.
    pub async fn resolve_admitted(
        &self,
        event: &BotEvent,
    ) -> Result<ResolvedConversationPolicy, BotAgentError> {
        self.resolve_admitted_with_config(event)
            .await
            .map(|(resolved, _)| resolved)
    }

    async fn resolve_admitted_with_config(
        &self,
        event: &BotEvent,
    ) -> Result<(ResolvedConversationPolicy, BotAgentConfig), BotAgentError> {
        let config = self.config.snapshot();
        if !config.enabled {
            return Err(BotAgentError::AgentDisabled);
        }
        let mut resolved = self.resolve(event).await?;
        if let Some(profile_id) = event
            .ext
            .get(BOT_EXT_AGENT_PROFILE_ID)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            resolved.policy.agent_runtime_profile_id = Some(profile_id.to_owned());
        }
        if resolved.policy.agent_runtime_profile_id.is_none()
            && !config.default_profile_id.trim().is_empty()
        {
            resolved.policy.agent_runtime_profile_id = Some(config.default_profile_id.clone());
        }
        Ok((resolved, config))
    }

    /// Reads or creates the current Agent session binding for an admitted event.
    ///
    /// # Errors
    ///
    /// Returns an error when admission, scope derivation, or repository access fails.
    pub async fn status(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve(event).await?;
        self.conversations
            .session_binding(&resolved, actor_id)
            .await?
            .ok_or(BotAgentError::SessionBindingMissing)
    }

    /// Cancels a turn and atomically advances the conversation binding.
    ///
    /// # Errors
    ///
    /// Returns a typed Agent or binding version error.
    pub async fn cancel(
        &self,
        resolved: &ResolvedConversationPolicy,
        binding: &AgentSessionBinding,
        actor_id: Option<&str>,
        turn_id: &str,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        let version = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .cancel_turn(
                &binding.session_id,
                turn_id,
                SessionVersion(binding.session_version),
            )?;
        Ok(self
            .conversations
            .advance_session(resolved, actor_id, binding.session_version, version.0)
            .await?)
    }

    /// Resolves an event and cancels the selected Agent turn.
    ///
    /// # Errors
    ///
    /// Returns a typed admission, Agent, or binding error.
    pub async fn cancel_event(
        &self,
        event: &BotEvent,
        turn_id: &str,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        let binding = self.status(event).await?;
        self.cancel(&resolved, &binding, actor_id, turn_id).await
    }

    /// Replaces the event's current session binding with a fresh session.
    ///
    /// # Errors
    ///
    /// Returns a typed admission or repository error.
    pub async fn reset(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        Ok(self
            .conversations
            .reset_session_binding(&resolved, actor_id)
            .await?)
    }

    /// Forks the event's session and commits the new binding atomically.
    ///
    /// # Errors
    ///
    /// Returns a typed admission, Agent, or binding conflict error.
    pub async fn fork(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event).await?;
        let (source, target_session_id) = self
            .conversations
            .prepare_session_fork(&resolved, actor_id)
            .await?;
        let target_version = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .fork_session(
                &source.session_id,
                &target_session_id,
                SessionVersion(source.session_version),
            )?;
        Ok(self
            .conversations
            .commit_session_fork(
                &resolved,
                actor_id,
                &source,
                target_session_id,
                target_version.0,
            )
            .await?)
    }

    /// Resubmits an event under a new idempotency identity for regeneration.
    ///
    /// # Errors
    ///
    /// Returns a typed status, admission, or Agent execution error.
    pub async fn regenerate(
        &self,
        event: &BotEvent,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        let binding = self.status(event).await?;
        let mut retry = event.clone();
        retry.event_id = format!("{}:regenerate:{}", event.event_id, binding.generation);
        self.submit_event(&retry).await
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BotAgentBridgeResult {
    pub resolved: ResolvedConversationPolicy,
    pub binding: AgentSessionBinding,
    pub turn_id: String,
    pub outgoing: Vec<BotMessage>,
}

fn event_message(
    event: &BotEvent,
    trace: Option<&BotAgentTraceContext>,
) -> Result<AgentMessage, BotAgentError> {
    let message = event
        .message
        .as_ref()
        .ok_or(BotAgentError::MessageMissing)?;
    let mut parts = Vec::new();
    for segment in &message.segments {
        match segment {
            MessageSegment::Image { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Image {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    width: None,
                    height: None,
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::Audio { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Audio {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    duration_ms: None,
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::File { resource, name } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Document {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    filename: name.clone(),
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::Video { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Document {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    filename: Some("qq-video".into()),
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            _ => {}
        }
    }
    let mut agent_message = AgentMessage::user(message.plain_text()).with_parts(parts);
    let mut metadata = serde_json::json!({
        "source": "qq",
        "account_id": &event.bot.account_id,
        "event_id": &event.event_id,
        "message_id": message.message_id.as_deref(),
        "reply_to": message.reply_to.as_deref(),
        "trace_id": trace.and_then(|trace| trace.trace_id.as_deref()),
        "correlation_id": trace.and_then(|trace| trace.correlation_id.as_deref()),
    });
    let mut injections = Vec::new();
    if let Some(icl) = event.ext.get(BOT_EXT_CONVERSATION_ICL) {
        metadata["icl"] = icl.clone();
        if let Ok(entries) = serde_json::from_value::<Vec<ConversationIclEntry>>(icl.clone()) {
            let summary = format_icl_summary(&entries);
            if !summary.is_empty() {
                injections.push(AgentMessageContextInjection {
                    source_kind: CONTEXT_SOURCE_ICL.into(),
                    text: summary,
                    source_id: resolved_origin_key(event),
                });
            }
        }
    }
    if let Some(identifiers) = event.ext.get(BOT_EXT_CONVERSATION_IDENTIFIERS) {
        metadata["identifiers"] = identifiers.clone();
        if let Ok(identifiers) =
            serde_json::from_value::<ConversationIdentifiers>(identifiers.clone())
            && let Some(text) = identifiers.prompt_text()
        {
            injections.push(AgentMessageContextInjection {
                source_kind: CONTEXT_SOURCE_IDENTIFIERS.into(),
                text,
                source_id: resolved_origin_key(event),
            });
        }
    }
    if let Some(prompt) = event
        .ext
        .get(BOT_EXT_PERSONA_PROMPT)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        metadata["persona_prompt"] = serde_json::Value::String(prompt.to_owned());
        injections.push(AgentMessageContextInjection {
            source_kind: CONTEXT_SOURCE_PERSONA.into(),
            text: prompt.to_owned(),
            source_id: "persona".into(),
        });
    }
    if !injections.is_empty() {
        metadata[AGENT_MESSAGE_CONTEXT_INJECTIONS_META] = serde_json::to_value(&injections)
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
    }
    agent_message.metadata = Some(metadata);
    Ok(agent_message)
}

fn resolved_origin_key(event: &BotEvent) -> String {
    qq_conversation_from_event(event)
        .map(|conversation| conversation.origin_key())
        .unwrap_or_else(|_| event.event_id.clone())
}

fn validate_agent_media(
    resource: &mutsuki_runtime_contracts::ResourceRef,
) -> Result<(), BotAgentError> {
    if resource.provider_id.trim().is_empty()
        || resource.schema.trim().is_empty()
        || resource.size_hint.is_none()
        || resource.content_hash.as_deref().is_none_or(str::is_empty)
    {
        return Err(BotAgentError::MediaResourceUnvalidated);
    }
    Ok(())
}

fn outgoing_messages_with_limit(
    source: &BotEvent,
    page: &AgentEventPage,
    strategy: &QqStreamingStrategy,
    max_message_bytes: usize,
) -> Vec<BotMessage> {
    let mut deltas = String::new();
    let mut final_text = None;
    let mut supplements = String::new();
    let mut artifacts = Vec::new();
    for envelope in &page.events {
        match &envelope.event {
            AgentEvent::ModelDelta { text, .. } => deltas.push_str(text),
            AgentEvent::FinalResponse { summary, .. } => final_text = Some(summary.clone()),
            AgentEvent::ArtifactProduced { artifact, .. } => {
                if let Some(resource) = artifact.content_ref.clone() {
                    if resource.size_hint.is_some()
                        && resource
                            .content_hash
                            .as_deref()
                            .is_some_and(|hash| !hash.is_empty())
                    {
                        let segment = if artifact.media_type.starts_with("image/") {
                            MessageSegment::Image { resource }
                        } else if artifact.media_type.starts_with("audio/") {
                            MessageSegment::Audio { resource }
                        } else if artifact.media_type.starts_with("video/") {
                            MessageSegment::Video { resource }
                        } else {
                            MessageSegment::File {
                                resource,
                                name: Some(artifact.artifact_id.clone()),
                            }
                        };
                        artifacts.push(BotMessage {
                            message_id: None,
                            target: source.target.clone(),
                            sender: None,
                            segments: vec![segment],
                            reply_to: None,
                            time_ms: None,
                            ext: std::collections::BTreeMap::default(),
                        });
                    } else {
                        let _ = write!(
                            supplements,
                            "\n\n[{}: unavailable resource]",
                            artifact.summary
                        );
                    }
                } else if !artifact.summary.is_empty() {
                    let _ = write!(supplements, "\n\n{}", artifact.summary);
                }
            }
            _ => {}
        }
    }
    let mut text = match strategy {
        QqStreamingStrategy::FinalOnly => final_text.unwrap_or(deltas),
        QqStreamingStrategy::SegmentMessages => {
            if deltas.is_empty() {
                final_text.unwrap_or_default()
            } else {
                deltas
            }
        }
    };
    text.push_str(&supplements);
    if text.is_empty() {
        return artifacts;
    }
    // QQ delivery has one size boundary regardless of whether the bridge emits deltas or a
    // final response. Final-only controls event selection, not an unlimited transport payload.
    let mut messages = split_text(&text, max_message_bytes)
        .into_iter()
        .map(|chunk| BotMessage::text(source.target.clone(), chunk))
        .collect::<Vec<_>>();
    messages.extend(artifacts);
    messages
}

fn trace_context(task: &Task) -> Option<BotAgentTraceContext> {
    (task.trace_id.is_some() || task.correlation_id.is_some()).then(|| BotAgentTraceContext {
        trace_id: task.trace_id.clone().map(Into::into),
        correlation_id: task.correlation_id.clone(),
    })
}

fn split_text(text: &str, max_bytes: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    if max_bytes < BOT_AGENT_MIN_MESSAGE_BYTES {
        return vec![text.to_owned()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(text[start..end].to_owned());
        start = end;
    }
    chunks
}

fn streaming_name(strategy: &QqStreamingStrategy) -> &'static str {
    match strategy {
        QqStreamingStrategy::FinalOnly => "final_only",
        QqStreamingStrategy::SegmentMessages => "segment_messages",
    }
}

#[derive(Debug, Error)]
pub enum BotAgentError {
    #[error(transparent)]
    Conversation(#[from] ConversationError),
    #[error("Agent client failed: {code}: {message}")]
    AgentClient { code: String, message: String },
    #[error("Agent is disabled for this conversation")]
    AgentDisabled,
    #[error("Bot Agent configuration is invalid: {0}")]
    Config(#[from] BotAgentConfigError),
    #[error("Agent runtime profile binding is missing")]
    AgentProfileMissing,
    #[error("Bot message is missing")]
    MessageMissing,
    #[error("Bot media ResourceRef is missing provider, MIME schema, size, or content hash")]
    MediaResourceUnvalidated,
    #[error("Agent client mutex was poisoned")]
    ClientPoisoned,
    #[error("Agent session binding is missing")]
    SessionBindingMissing,
    #[error("Agent command is invalid or unsupported")]
    InvalidCommand,
}

impl From<AgentWireError> for BotAgentError {
    fn from(error: AgentWireError) -> Self {
        Self::AgentClient {
            code: error.code,
            message: error.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use async_trait::async_trait;
    use mutsuki_agent_contracts::{
        AgentEventEnvelope, AgentEventMeta, ArtifactRef, ResourceCellRef, ResourceRef,
    };
    use mutsuki_bot_conversation::ConversationRepository;
    use mutsuki_bot_protocol::{
        AgentSessionScope, BotAccountRef, BotEventKind, BotPlatform, BotTarget, BotUser,
        ConversationPolicy,
    };
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        futures::executor::block_on(future)
    }

    #[derive(Default)]
    struct Repository {
        bindings: Mutex<BTreeMap<String, AgentSessionBinding>>,
        events: Mutex<BTreeMap<(String, String), bool>>,
    }

    #[async_trait]
    impl ConversationRepository for Repository {
        async fn session_binding(
            &self,
            key: &str,
        ) -> Result<Option<AgentSessionBinding>, ConversationError> {
            Ok(self.bindings.lock().unwrap().get(key).cloned())
        }

        async fn compare_and_set_session_binding(
            &self,
            key: &str,
            expected: Option<u64>,
            binding: AgentSessionBinding,
        ) -> Result<(), ConversationError> {
            let mut bindings = self.bindings.lock().unwrap();
            if bindings.get(key).map(|value| value.generation) != expected {
                return Err(ConversationError::GenerationConflict);
            }
            bindings.insert(key.into(), binding);
            Ok(())
        }

        async fn begin_agent_event(
            &self,
            binding_key: &str,
            event_id: &str,
            _turn_id: &str,
        ) -> Result<AgentEventClaim, ConversationError> {
            let mut events = self.events.lock().unwrap();
            let key = (binding_key.into(), event_id.into());
            Ok(match events.get(&key) {
                Some(true) => AgentEventClaim::Completed,
                Some(false) => AgentEventClaim::ResumePending,
                None => {
                    events.insert(key, false);
                    AgentEventClaim::New
                }
            })
        }

        async fn complete_agent_event(
            &self,
            binding_key: &str,
            event_id: &str,
        ) -> Result<(), ConversationError> {
            self.events
                .lock()
                .unwrap()
                .insert((binding_key.into(), event_id.into()), true);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeAgentClient {
        sessions: BTreeSet<String>,
        versions: BTreeMap<String, u64>,
        next_events: BTreeMap<String, AgentEventPage>,
        submitted: Vec<(String, String)>,
        started_profiles: Arc<Mutex<Vec<String>>>,
    }

    impl AgentBridgeClient for FakeAgentClient {
        fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
            if !self.sessions.contains(session_id) {
                return Err(wire_error("agent.session.not_found"));
            }
            Ok(session(session_id))
        }

        fn start_session(
            &mut self,
            request: AgentSessionCreateRequest,
        ) -> Result<AgentSession, AgentWireError> {
            let id = request.session_id.unwrap();
            self.started_profiles
                .lock()
                .unwrap()
                .push(request.profile_id.clone());
            self.sessions.insert(id.clone());
            self.versions.insert(id.clone(), 0);
            Ok(session(&id))
        }

        fn submit_turn(
            &mut self,
            session_id: &str,
            expected_version: SessionVersion,
            turn_id: &str,
            _messages: Vec<AgentMessage>,
            idempotency_key: &str,
        ) -> Result<SessionVersion, AgentWireError> {
            assert_eq!(self.versions[session_id], expected_version.0);
            let next = expected_version.0 + 1;
            self.versions.insert(session_id.into(), next);
            self.submitted
                .push((session_id.into(), idempotency_key.into()));
            self.next_events.insert(
                session_id.into(),
                AgentEventPage {
                    events: vec![AgentEventEnvelope {
                        session_id: session_id.into(),
                        sequence: next,
                        meta: AgentEventMeta::default(),
                        event: AgentEvent::FinalResponse {
                            turn_id: turn_id.into(),
                            summary: format!("reply-{next}"),
                            result: None,
                        },
                    }],
                    next_sequence: next,
                    lost: 0,
                    truncated: false,
                },
            );
            Ok(SessionVersion(next))
        }

        fn cancel_turn(
            &mut self,
            session_id: &str,
            _turn_id: &str,
            expected_version: SessionVersion,
        ) -> Result<SessionVersion, AgentWireError> {
            let next = expected_version.0 + 1;
            self.versions.insert(session_id.into(), next);
            Ok(SessionVersion(next))
        }

        fn fork_session(
            &mut self,
            source_session_id: &str,
            target_session_id: &str,
            expected_version: SessionVersion,
        ) -> Result<SessionVersion, AgentWireError> {
            assert_eq!(self.versions[source_session_id], expected_version.0);
            self.sessions.insert(target_session_id.into());
            self.versions.insert(target_session_id.into(), 1);
            Ok(SessionVersion(1))
        }

        fn events(
            &mut self,
            session_id: &str,
            _after_sequence: u64,
        ) -> Result<AgentEventPage, AgentWireError> {
            Ok(self.next_events.remove(session_id).unwrap())
        }
    }

    #[derive(Default)]
    struct ReplayableAgentState {
        sessions: BTreeSet<String>,
        versions: BTreeMap<String, u64>,
        idempotency: BTreeMap<String, SessionVersion>,
        pages: BTreeMap<String, AgentEventPage>,
        submit_count: usize,
        event_calls: usize,
        fail_first_events: bool,
    }

    #[derive(Clone)]
    struct ReplayableAgentClient {
        state: Arc<Mutex<ReplayableAgentState>>,
    }

    impl AgentBridgeClient for ReplayableAgentClient {
        fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
            let state = self.state.lock().unwrap();
            state
                .sessions
                .contains(session_id)
                .then(|| session(session_id))
                .ok_or_else(|| wire_error("agent.session.not_found"))
        }

        fn start_session(
            &mut self,
            request: AgentSessionCreateRequest,
        ) -> Result<AgentSession, AgentWireError> {
            let session_id = request
                .session_id
                .ok_or_else(|| wire_error("agent.session.id_missing"))?;
            let mut state = self.state.lock().unwrap();
            state.sessions.insert(session_id.clone());
            state.versions.insert(session_id.clone(), 0);
            Ok(session(&session_id))
        }

        fn submit_turn(
            &mut self,
            session_id: &str,
            expected_version: SessionVersion,
            turn_id: &str,
            _messages: Vec<AgentMessage>,
            idempotency_key: &str,
        ) -> Result<SessionVersion, AgentWireError> {
            let mut state = self.state.lock().unwrap();
            if let Some(version) = state.idempotency.get(idempotency_key) {
                return Ok(*version);
            }
            if state.versions.get(session_id).copied() != Some(expected_version.0) {
                return Err(wire_error("agent.session.version_conflict"));
            }
            let next = SessionVersion(expected_version.0 + 1);
            state.versions.insert(session_id.into(), next.0);
            state.idempotency.insert(idempotency_key.into(), next);
            state.submit_count += 1;
            state.pages.insert(
                session_id.into(),
                AgentEventPage {
                    events: vec![AgentEventEnvelope {
                        session_id: session_id.into(),
                        sequence: next.0,
                        meta: AgentEventMeta::default(),
                        event: AgentEvent::FinalResponse {
                            turn_id: turn_id.into(),
                            summary: "recovered reply".into(),
                            result: None,
                        },
                    }],
                    next_sequence: next.0,
                    lost: 0,
                    truncated: false,
                },
            );
            Ok(next)
        }

        fn cancel_turn(
            &mut self,
            session_id: &str,
            _turn_id: &str,
            expected_version: SessionVersion,
        ) -> Result<SessionVersion, AgentWireError> {
            let mut state = self.state.lock().unwrap();
            if state.versions.get(session_id).copied() != Some(expected_version.0) {
                return Err(wire_error("agent.session.version_conflict"));
            }
            let next = expected_version.0 + 1;
            state.versions.insert(session_id.into(), next);
            Ok(SessionVersion(next))
        }

        fn fork_session(
            &mut self,
            _source_session_id: &str,
            _target_session_id: &str,
            _expected_version: SessionVersion,
        ) -> Result<SessionVersion, AgentWireError> {
            Err(wire_error("agent.session.fork_not_used"))
        }

        fn events(
            &mut self,
            session_id: &str,
            after_sequence: u64,
        ) -> Result<AgentEventPage, AgentWireError> {
            let mut state = self.state.lock().unwrap();
            state.event_calls += 1;
            if state.fail_first_events && state.event_calls == 1 {
                return Err(wire_error("agent.transport.disconnected"));
            }
            let mut page = state
                .pages
                .get(session_id)
                .cloned()
                .unwrap_or(AgentEventPage {
                    events: Vec::new(),
                    next_sequence: after_sequence,
                    lost: 0,
                    truncated: false,
                });
            page.events.retain(|event| event.sequence > after_sequence);
            page.next_sequence = page
                .events
                .last()
                .map_or(after_sequence, |event| event.sequence);
            Ok(page)
        }
    }

    #[test]
    fn private_and_group_two_turns_reuse_their_stable_sessions() {
        for target in [
            BotTarget::User {
                user_id: "user".into(),
            },
            BotTarget::Group {
                group_id: "group".into(),
            },
        ] {
            let repository = Arc::new(Repository::default());
            let bridge = BotAgentBridge::new(
                ConversationService::new(repository, policy()),
                Box::new(FakeAgentClient::default()),
                &QqStreamingStrategy::FinalOnly,
            );
            let first = block_on(bridge.submit_event(&event("one", target.clone()))).unwrap();
            let second = block_on(bridge.submit_event(&event("two", target))).unwrap();
            assert_eq!(first.binding.session_id, second.binding.session_id);
            assert_eq!(second.binding.session_version, 2);
            assert_eq!(second.binding.last_event_sequence, 2);
            assert_eq!(second.outgoing[0].plain_text(), "reply-2");
        }
    }

    #[test]
    fn configured_default_profile_is_used_when_policy_does_not_bind_one() {
        let repository = Arc::new(Repository::default());
        let mut policy = policy();
        policy.agent_runtime_profile_id = None;
        let config = BotAgentConfigHandle::new(BotAgentConfig {
            enabled: true,
            connection_id: "injected".into(),
            default_profile_id: "configured-profile".into(),
            ..BotAgentConfig::default()
        })
        .unwrap();
        let started_profiles = Arc::new(Mutex::new(Vec::new()));
        let bridge = BotAgentBridge::new_with_config(
            ConversationService::new(repository, policy),
            Box::new(FakeAgentClient {
                started_profiles: started_profiles.clone(),
                ..FakeAgentClient::default()
            }),
            config,
        );

        let result = block_on(bridge.submit_event(&event(
            "configured-profile",
            BotTarget::User {
                user_id: "actor".into(),
            },
        )))
        .unwrap();
        assert_eq!(
            result.resolved.policy.agent_runtime_profile_id.as_deref(),
            Some("configured-profile")
        );
        assert_eq!(
            started_profiles.lock().unwrap().as_slice(),
            ["configured-profile"]
        );
    }

    #[test]
    fn event_ext_profile_overlays_resolved_policy_ahead_of_config_default() {
        let repository = Arc::new(Repository::default());
        let mut policy = policy();
        policy.agent_runtime_profile_id = None;
        let config = BotAgentConfigHandle::new(BotAgentConfig {
            enabled: true,
            connection_id: "injected".into(),
            default_profile_id: "configured-profile".into(),
            ..BotAgentConfig::default()
        })
        .unwrap();
        let started_profiles = Arc::new(Mutex::new(Vec::new()));
        let bridge = BotAgentBridge::new_with_config(
            ConversationService::new(repository, policy),
            Box::new(FakeAgentClient {
                started_profiles: started_profiles.clone(),
                ..FakeAgentClient::default()
            }),
            config,
        );
        let mut source = event(
            "ext-profile",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        source.ext.insert(
            BOT_EXT_AGENT_PROFILE_ID.into(),
            serde_json::Value::String("persona-profile".into()),
        );

        let result = block_on(bridge.submit_event(&source)).unwrap();
        assert_eq!(
            result.resolved.policy.agent_runtime_profile_id.as_deref(),
            Some("persona-profile")
        );
        assert_eq!(
            started_profiles.lock().unwrap().as_slice(),
            ["persona-profile"]
        );
    }

    #[test]
    fn bind_profile_fills_missing_ext_and_does_not_overwrite_persona() {
        let source = event(
            "bind-profile",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        let filled =
            bind_profile_event(&source, serde_json::json!({"profile_id": "config-profile"}));
        assert_eq!(
            filled.ext[BOT_EXT_AGENT_PROFILE_ID],
            serde_json::Value::String("config-profile".into())
        );

        let mut with_persona = source.clone();
        with_persona.ext.insert(
            BOT_EXT_AGENT_PROFILE_ID.into(),
            serde_json::Value::String("persona-profile".into()),
        );
        let preserved = bind_profile_event(
            &with_persona,
            serde_json::json!({"profile_id": "config-profile"}),
        );
        assert_eq!(
            preserved.ext[BOT_EXT_AGENT_PROFILE_ID],
            serde_json::Value::String("persona-profile".into())
        );

        let overridden = bind_profile_event(
            &with_persona,
            serde_json::json!({"profile_id": "config-profile", "override": true}),
        );
        assert_eq!(
            overridden.ext[BOT_EXT_AGENT_PROFILE_ID],
            serde_json::Value::String("config-profile".into())
        );

        let empty_config = bind_profile_event(&source, serde_json::json!({}));
        assert!(!empty_config.ext.contains_key(BOT_EXT_AGENT_PROFILE_ID));
    }

    #[test]
    fn submit_reply_envelope_carries_source_message_and_user_without_reserving() {
        let mut source = event(
            "reply-source",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        source.message.as_mut().unwrap().message_id = Some("mid-1".into());
        let context = overlay_reply_source_context(
            BotFlowContext {
                bot: None,
                target: None,
                actor: None,
                ext: BTreeMap::default(),
            },
            &source,
        );
        assert_eq!(
            context.ext[BOT_EXT_REPLY_SOURCE_MESSAGE_ID],
            serde_json::Value::String("mid-1".into())
        );
        assert_eq!(
            context.ext[BOT_EXT_REPLY_SOURCE_USER_ID],
            serde_json::Value::String("actor".into())
        );
    }

    #[test]
    fn globally_disabled_bridge_rejects_before_creating_a_session() {
        let repository = Arc::new(Repository::default());
        let config = BotAgentConfigHandle::new(BotAgentConfig {
            enabled: false,
            ..BotAgentConfig::default()
        })
        .unwrap();
        let bridge = BotAgentBridge::new_with_config(
            ConversationService::new(repository.clone(), policy()),
            Box::new(FakeAgentClient::default()),
            config,
        );

        assert!(matches!(
            block_on(bridge.submit_event(&event(
                "disabled",
                BotTarget::Group {
                    group_id: "group".into(),
                },
            ))),
            Err(BotAgentError::AgentDisabled)
        ));
        assert!(repository.bindings.lock().unwrap().is_empty());
    }

    #[test]
    fn configured_concurrency_gate_releases_after_invocation_drop() {
        let gate = Arc::new(BotAgentConcurrencyGate::default());
        let permit = gate.try_acquire(1).expect("first invocation is admitted");
        assert!(gate.try_acquire(1).is_none());
        drop(permit);
        assert!(gate.try_acquire(1).is_some());
    }

    #[test]
    fn final_response_keeps_non_resource_artifact_summary() {
        let source = event(
            "artifact",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        let page = AgentEventPage {
            events: vec![
                AgentEventEnvelope {
                    session_id: "session".into(),
                    sequence: 1,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::FinalResponse {
                        turn_id: "turn".into(),
                        summary: "answer".into(),
                        result: None,
                    },
                },
                AgentEventEnvelope {
                    session_id: "session".into(),
                    sequence: 2,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::ArtifactProduced {
                        turn_id: "turn".into(),
                        artifact: ArtifactRef {
                            artifact_id: "citation".into(),
                            media_type: "text/plain".into(),
                            summary: "citation summary".into(),
                            content_ref: None,
                            kind: Some("citation".into()),
                            size_bytes: None,
                            content_hash: None,
                            provenance: None,
                            open_hint: None,
                            action_hint: None,
                        },
                    },
                },
            ],
            next_sequence: 2,
            lost: 0,
            truncated: false,
        };

        let outgoing = outgoing_messages_with_limit(
            &source,
            &page,
            &QqStreamingStrategy::FinalOnly,
            BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
        );

        assert_eq!(outgoing[0].plain_text(), "answer\n\ncitation summary");
    }

    #[test]
    fn final_only_reply_is_segmented_at_the_configured_transport_boundary() {
        let source = event(
            "long-reply",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        let page = AgentEventPage {
            events: vec![AgentEventEnvelope {
                session_id: "session".into(),
                sequence: 1,
                meta: AgentEventMeta::default(),
                event: AgentEvent::FinalResponse {
                    turn_id: "turn".into(),
                    summary: "abcdefghij".into(),
                    result: None,
                },
            }],
            next_sequence: 1,
            lost: 0,
            truncated: false,
        };

        let outgoing =
            outgoing_messages_with_limit(&source, &page, &QqStreamingStrategy::FinalOnly, 4);
        assert_eq!(
            outgoing
                .iter()
                .map(BotMessage::plain_text)
                .collect::<Vec<_>>(),
            vec!["abcd", "efgh", "ij"]
        );
    }

    #[test]
    fn segmented_streaming_uses_model_deltas_while_final_only_uses_final_response() {
        let source = event(
            "streaming",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        let page = AgentEventPage {
            events: vec![
                AgentEventEnvelope {
                    session_id: "session".into(),
                    sequence: 1,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::ModelDelta {
                        turn_id: "turn".into(),
                        text: "partial ".into(),
                    },
                },
                AgentEventEnvelope {
                    session_id: "session".into(),
                    sequence: 2,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::ModelDelta {
                        turn_id: "turn".into(),
                        text: "reply".into(),
                    },
                },
                AgentEventEnvelope {
                    session_id: "session".into(),
                    sequence: 3,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::FinalResponse {
                        turn_id: "turn".into(),
                        summary: "final reply".into(),
                        result: None,
                    },
                },
            ],
            next_sequence: 3,
            lost: 0,
            truncated: false,
        };

        let final_only = outgoing_messages_with_limit(
            &source,
            &page,
            &QqStreamingStrategy::FinalOnly,
            BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
        );
        let segmented = outgoing_messages_with_limit(
            &source,
            &page,
            &QqStreamingStrategy::SegmentMessages,
            BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
        );
        assert_eq!(final_only[0].plain_text(), "final reply");
        assert_eq!(segmented[0].plain_text(), "partial reply");
    }

    #[test]
    fn completed_event_is_suppressed_before_agent_submit_after_bridge_reload() {
        let repository = Arc::new(Repository::default());
        let event = event(
            "duplicate",
            BotTarget::Group {
                group_id: "group".into(),
            },
        );
        let first_bridge = BotAgentBridge::new(
            ConversationService::new(repository.clone(), policy()),
            Box::new(FakeAgentClient::default()),
            &QqStreamingStrategy::FinalOnly,
        );
        let first = block_on(first_bridge.submit_event(&event)).unwrap();
        assert_eq!(first.binding.session_version, 1);
        let reloaded_bridge = BotAgentBridge::new(
            ConversationService::new(repository, policy()),
            Box::new(FakeAgentClient::default()),
            &QqStreamingStrategy::FinalOnly,
        );
        let duplicate = block_on(reloaded_bridge.submit_event(&event)).unwrap();
        assert_eq!(duplicate.binding.session_version, 1);
        assert!(duplicate.outgoing.is_empty());
    }

    #[test]
    fn pending_event_resumes_after_bridge_reload_without_duplicate_submit() {
        let repository = Arc::new(Repository::default());
        let state = Arc::new(Mutex::new(ReplayableAgentState {
            fail_first_events: true,
            ..ReplayableAgentState::default()
        }));
        let event = event(
            "pending-reconnect",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        let first_bridge = BotAgentBridge::new(
            ConversationService::new(repository.clone(), policy()),
            Box::new(ReplayableAgentClient {
                state: state.clone(),
            }),
            &QqStreamingStrategy::FinalOnly,
        );
        assert!(matches!(
            block_on(first_bridge.submit_event(&event)),
            Err(BotAgentError::AgentClient { ref code, .. })
                if code == "agent.transport.disconnected"
        ));

        let reloaded_bridge = BotAgentBridge::new(
            ConversationService::new(repository, policy()),
            Box::new(ReplayableAgentClient {
                state: state.clone(),
            }),
            &QqStreamingStrategy::FinalOnly,
        );
        let recovered = block_on(reloaded_bridge.submit_event(&event)).unwrap();
        assert_eq!(recovered.outgoing[0].plain_text(), "recovered reply");
        let state = state.lock().unwrap();
        assert_eq!(state.submit_count, 1);
        assert_eq!(state.event_calls, 2);
    }

    #[test]
    fn cancel_fork_reset_and_regenerate_have_fenced_session_transitions() {
        let repository = Arc::new(Repository::default());
        let bridge = BotAgentBridge::new(
            ConversationService::new(repository, policy()),
            Box::new(FakeAgentClient::default()),
            &QqStreamingStrategy::FinalOnly,
        );
        let event = event(
            "actions",
            BotTarget::Group {
                group_id: "group".into(),
            },
        );
        let submitted = block_on(bridge.submit_event(&event)).unwrap();
        let cancelled = block_on(bridge.cancel_event(&event, &submitted.turn_id)).unwrap();
        assert_eq!(cancelled.session_version, 2);
        let forked = block_on(bridge.fork(&event)).unwrap();
        assert_ne!(forked.session_id, cancelled.session_id);
        assert_eq!(forked.session_version, 1);
        assert!(forked.generation > cancelled.generation);
        let reset = block_on(bridge.reset(&event)).unwrap();
        assert_ne!(reset.session_id, submitted.binding.session_id);
        assert_eq!(reset.session_version, 0);
        assert!(reset.generation > forked.generation);
        let regenerated = block_on(bridge.regenerate(&event)).unwrap();
        assert_eq!(regenerated.binding.session_id, reset.session_id);
        assert_eq!(regenerated.binding.session_version, 1);
        assert!(regenerated.turn_id.contains(":regenerate:"));
    }

    #[test]
    fn validated_image_and_trace_metadata_enter_the_public_agent_message() {
        let mut event = event(
            "media",
            BotTarget::User {
                user_id: "user".into(),
            },
        );
        event
            .message
            .as_mut()
            .unwrap()
            .segments
            .push(MessageSegment::Image {
                resource: media_resource("image", "image/png"),
            });
        let message = event_message(
            &event,
            Some(&BotAgentTraceContext {
                trace_id: Some("trace".into()),
                correlation_id: Some("correlation".into()),
            }),
        )
        .unwrap();
        assert!(matches!(message.parts[0], AgentContentPart::Image { .. }));
        assert_eq!(message.metadata.as_ref().unwrap()["event_id"], "media");
        assert_eq!(message.metadata.as_ref().unwrap()["trace_id"], "trace");

        let MessageSegment::Image { resource } =
            event.message.as_mut().unwrap().segments.last_mut().unwrap()
        else {
            unreachable!()
        };
        resource.content_hash = None;
        assert!(matches!(
            event_message(&event, None),
            Err(BotAgentError::MediaResourceUnvalidated)
        ));
    }

    #[test]
    fn conversation_injections_stay_in_metadata_not_user_text() {
        let mut event = event(
            "icl",
            BotTarget::Group {
                group_id: "group".into(),
            },
        );
        event.ext.insert(
            BOT_EXT_CONVERSATION_ICL.into(),
            serde_json::to_value([ConversationIclEntry {
                actor_id: "alice".into(),
                display_name: Some("Alice".into()),
                text: "hello group".into(),
                time_ms: 1,
            }])
            .unwrap(),
        );
        event.ext.insert(
            BOT_EXT_PERSONA_PROMPT.into(),
            serde_json::Value::String("you are a helper".into()),
        );
        let message = event_message(&event, None).unwrap();
        assert_eq!(
            message.content,
            event.message.as_ref().unwrap().plain_text()
        );
        assert!(message.parts.iter().all(|part| !matches!(
            part,
            AgentContentPart::Text { text } if text.contains("hello group") || text.contains("you are a helper")
        )));
        let injections: Vec<AgentMessageContextInjection> = serde_json::from_value(
            message.metadata.as_ref().unwrap()[AGENT_MESSAGE_CONTEXT_INJECTIONS_META].clone(),
        )
        .unwrap();
        assert!(
            injections
                .iter()
                .any(|injection| injection.source_kind == CONTEXT_SOURCE_ICL
                    && injection.text.contains("hello group"))
        );
        assert!(
            injections
                .iter()
                .any(|injection| injection.source_kind == CONTEXT_SOURCE_PERSONA
                    && injection.text == "you are a helper")
        );
    }

    fn bind_profile_event(source: &BotEvent, config: serde_json::Value) -> BotEvent {
        let task = Task::new(
            "bind-profile",
            BOT_AGENT_BIND_PROFILE_PROTOCOL_ID,
            serde_json::json!({}),
        );
        let invocation = BotNodeInvocation {
            flow_id: "flow".into(),
            graph_revision: 1,
            execution_id: "ex".into(),
            node_id: "bind".into(),
            input_port_id: "input".into(),
            wiring: BotNodeWiring::default(),
            config,
            input: BotFlowEventEnvelope {
                event_id: source.event_id.clone(),
                protocol_id: BOT_FLOW_BOT_EVENT_TYPE.into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                    value: serde_json::to_value(source).unwrap(),
                },
                context: BotFlowContext {
                    bot: None,
                    target: None,
                    actor: None,
                    ext: BTreeMap::default(),
                },
                trace_id: None,
                correlation_id: None,
            },
        };
        let result = bind_profile_node(&task, invocation).unwrap();
        let node: BotNodeResult = serde_json::from_value(result.output.unwrap()).unwrap();
        serde_json::from_value(node.outputs[0].event.payload.value.clone()).unwrap()
    }

    fn event(id: &str, target: BotTarget) -> BotEvent {
        BotEvent {
            event_id: id.into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: target.clone(),
            actor: Some(BotUser {
                user_id: "actor".into(),
                display_name: None,
                avatar_url: None,
            }),
            message: Some(BotMessage::text(target, "hello")),
            raw: None,
            ext: BTreeMap::new(),
        }
    }

    fn policy() -> ConversationPolicy {
        ConversationPolicy {
            revision: 1,
            session_scope: AgentSessionScope::SharedConversation,
            business_profile_binding_id: None,
            agent_runtime_profile_id: Some("profile".into()),
            stt_enabled: false,
            tts_enabled: false,
            speech_reply_policy: BotSpeechReplyPolicy::default(),
            stt_selector_id: None,
            tts_selector_id: None,
            active_delivery_enabled: false,
        }
    }

    fn session(id: &str) -> AgentSession {
        AgentSession::new(
            id,
            "profile",
            resource(id),
            ResourceCellRef {
                cell_id: format!("cell-{id}").into(),
                resource_kind: "agent.session".into(),
                owner_plugin_id: "test".into(),
                schema: "agent.session.v1".into(),
                generation: 1,
                health: "ready".into(),
                reload_policy: "retain".into(),
            },
        )
    }

    fn resource(id: &str) -> ResourceRef {
        ResourceRef {
            ref_id: format!("ref-{id}").into(),
            resource_id: ResourceId {
                kind_id: "agent.session".into(),
                slot_id: id.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::VersionedSnapshot,
            provider_id: "test".into(),
            resource_kind: "agent.session".into(),
            schema: "agent.session.v1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn media_resource(id: &str, schema: &str) -> ResourceRef {
        let mut resource = resource(id);
        resource.resource_kind = "blob".into();
        resource.schema = schema.into();
        resource.size_hint = Some(4);
        resource.content_hash = Some(format!("sha256:{id}"));
        resource
    }

    fn wire_error(code: &str) -> AgentWireError {
        AgentWireError {
            code: code.into(),
            message: code.into(),
            retryable: false,
        }
    }
}
