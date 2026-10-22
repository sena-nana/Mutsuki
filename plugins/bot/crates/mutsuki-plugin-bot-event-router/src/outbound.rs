//! The account-routed outbound protocols and the platform-neutral sink nodes.
//!
//! `message/send@2`, `message/recall@2` and `media/upload@2` are served by exactly one runner:
//! this router. The Host admits one runner per protocol, so adapters cannot all accept the
//! neutral id; each adapter instead publishes an endpoint in its `BotAdapterDescriptor`, and the
//! router calls that endpoint by `runner_hint` after resolving the account against the adapter
//! directory the Flow `LoadPlan` observer published on `BotFlowRegistry`.

use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_protocol::{
    BOT_FLOW_MESSAGE_EVENT_TYPE, BOT_FLOW_MESSAGE_SEND_TYPE, BOT_MEDIA_UPLOAD_V2_PROTOCOL_ID,
    BOT_MESSAGE_RECALL_V2_PROTOCOL_ID, BOT_MESSAGE_SEND_V2_PROTOCOL_ID, BotAccountRequest,
    BotFlowTypeRef, BotNodeBinding, BotNodeDescriptor, BotNodeInvocation, BotNodePortDescriptor,
    BotNodePortDirection, BotNodeResult, BotNodeRole, BotOutboundOperation,
};
use mutsuki_bot_sdk::outbound::{invocation_account, send_message_from_payload};
use mutsuki_runtime_contracts::{
    ExecutionClass, InvocationMode, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerDescriptor, RunnerMode, RunnerResult, RunnerSideEffect,
    RunnerStatus, RuntimeError, Task, TaskOutcome, TimeoutGranularity,
};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, Runner, RunnerDescriptorBuilder, RuntimeClientRef, RuntimeFailure,
    RuntimeResult,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::BOT_FLOW_ROUTER_PLUGIN_ID;
use crate::plan_bound::PlanBoundRunner;

pub const BOT_FLOW_OUTBOUND_RUNNER_ID: &str = "mutsuki.bot.router.flow.outbound";
pub const BOT_SEND_NODE_TYPE: &str = "mutsuki.bot.send";
pub const BOT_RECALL_NODE_TYPE: &str = "mutsuki.bot.recall";
pub const BOT_UPLOAD_NODE_TYPE: &str = "mutsuki.bot.upload";

pub const OUTBOUND_PROTOCOL_IDS: &[&str] = &[
    BOT_MESSAGE_SEND_V2_PROTOCOL_ID,
    BOT_MESSAGE_RECALL_V2_PROTOCOL_ID,
    BOT_MEDIA_UPLOAD_V2_PROTOCOL_ID,
];

/// The router as declared in the manifest: no endpoint is required statically, because which
/// adapters are loaded is a `LoadPlan` fact.
#[must_use]
pub fn outbound_descriptor() -> RunnerDescriptor {
    outbound_descriptor_for(&[])
}

/// The router as constructed for one plan: it may call every endpoint the plan's adapters serve.
fn outbound_descriptor_for(endpoint_protocols: &[String]) -> RunnerDescriptor {
    let mut builder =
        RunnerDescriptorBuilder::new(BOT_FLOW_OUTBOUND_RUNNER_ID, BOT_FLOW_ROUTER_PLUGIN_ID);
    for protocol in OUTBOUND_PROTOCOL_IDS {
        builder = builder.accepted_protocol(*protocol);
    }
    endpoint_protocols
        .iter()
        .fold(builder, |builder, protocol| {
            builder.requires_protocol(protocol.clone())
        })
        .execution_class(ExecutionClass::Orchestration)
        .invocation_mode(InvocationMode::SyncExclusive)
        .concurrency(RunnerConcurrency::Sharded { instances: 1 })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 32,
            max_batch_entries: 128,
            max_entry_concurrency: 128,
            max_inflight_batches: 1,
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

/// The router. Its descriptor is the manifest one; the endpoints it may call are a `LoadPlan` fact.
pub fn flow_outbound_runner(
    client: RuntimeClientRef,
    registry: Arc<BotFlowRegistry>,
) -> Box<dyn Runner> {
    let call_registry = registry.clone();
    Box::new(PlanBoundRunner::new(
        outbound_descriptor(),
        client,
        registry,
        |registry| outbound_descriptor_for(&registry.adapters().endpoint_protocol_ids()),
        Arc::new(move |ctx, task| Box::pin(route_outbound(ctx, task, call_registry.clone()))),
    ))
}

async fn route_outbound(
    ctx: AsyncRunnerContext,
    task: Task,
    registry: Arc<BotFlowRegistry>,
) -> RuntimeResult<RunnerResult> {
    let operation = BotOutboundOperation::from_routed_protocol_id(task.protocol_id.as_str())
        .ok_or_else(|| failure(&task, "protocol", &task.protocol_id))?;
    let payload = task.payload.to_value();
    // A sink node hands over its invocation; direct callers already send the routed request.
    let invocation = BotNodeInvocation::deserialize(&payload).ok();
    let request = match &invocation {
        Some(invocation) => node_request(operation, invocation)
            .map_err(|error| failure(&task, "node_request", error))?,
        None => BotAccountRequest::<Value>::deserialize(&payload)
            .map_err(|error| failure(&task, "decode", error))?,
    };
    let route = registry
        .adapters()
        .route(operation, &request.account)
        .map_err(|error| failure(&task, "route", error))?;
    let body = serde_json::to_value(&request).map_err(|error| failure(&task, "encode", error))?;
    let outcome = ctx
        .call_targeted_raw(
            format!("binding:{}", route.endpoint.protocol_id),
            route.endpoint.protocol_id,
            route.endpoint.runner_id,
            body,
        )
        .await?;
    // The endpoint's verdict is the caller's: a failure keeps its code and route (delivery
    // classifies retries and reconciliation by them) and a cancellation stays a cancellation.
    let output = match outcome.into() {
        TaskOutcome::Completed { output, .. } => output.unwrap_or(Value::Null),
        TaskOutcome::Failed { error, .. } => return Err(RuntimeFailure::new(error)),
        TaskOutcome::Cancelled { .. } => {
            let mut cancelled = RunnerResult::completed(task.task_id.clone());
            cancelled.status = RunnerStatus::Cancelled;
            return Ok(cancelled);
        }
        TaskOutcome::Expired { .. } => return Err(failure(&task, "expired", "expired")),
        TaskOutcome::DeadLetter { .. } => {
            return Err(failure(&task, "dead_letter", "dead letter"));
        }
    };
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(if invocation.is_some() {
        serde_json::to_value(BotNodeResult {
            outputs: Vec::new(),
            metadata: BTreeMap::from([("receipt".into(), output)]),
        })
        .map_err(|error| failure(&task, "node_result", error))?
    } else {
        output
    });
    Ok(result)
}

/// The routed request a sink node stands for: the account the triggering event arrived on and
/// the node's input, with the send node's `text`/`reply` config applied.
fn node_request(
    operation: BotOutboundOperation,
    invocation: &BotNodeInvocation,
) -> Result<BotAccountRequest<Value>, String> {
    let account = invocation_account(invocation).ok_or_else(|| {
        "bot.outbound.account_missing: the input carries no bot account to route by".to_owned()
    })?;
    let payload = &invocation.input.payload.value;
    let request = match operation {
        BotOutboundOperation::Send => serde_json::to_value(
            send_message_from_payload(payload, Some(invocation))
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?,
        BotOutboundOperation::Recall | BotOutboundOperation::Upload => payload.clone(),
    };
    Ok(BotAccountRequest { account, request })
}

/// Platform-neutral sinks bound to the account-routed protocols.
pub fn neutral_sink_nodes() -> Vec<BotNodeDescriptor> {
    vec![
        BotNodeDescriptor {
            node_type_id: BOT_SEND_NODE_TYPE.into(),
            version: 1,
            title: "发送消息".into(),
            category: "消息".into(),
            role: BotNodeRole::Sink,
            binding: Some(outbound_binding(BOT_MESSAGE_SEND_V2_PROTOCOL_ID)),
            ports: vec![
                input_port("input", "消息", BOT_FLOW_MESSAGE_SEND_TYPE, false),
                input_port("event", "消息事件", BOT_FLOW_MESSAGE_EVENT_TYPE, false),
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
        },
        BotNodeDescriptor {
            node_type_id: BOT_RECALL_NODE_TYPE.into(),
            version: 1,
            title: "撤回消息".into(),
            category: "消息".into(),
            role: BotNodeRole::Sink,
            binding: Some(outbound_binding(BOT_MESSAGE_RECALL_V2_PROTOCOL_ID)),
            ports: vec![input_port(
                "input",
                "输入",
                "mutsuki.bot.message.recall",
                true,
            )],
            config_schema: json!({"type": "object", "additionalProperties": false}),
        },
        BotNodeDescriptor {
            node_type_id: BOT_UPLOAD_NODE_TYPE.into(),
            version: 1,
            title: "上传媒体".into(),
            category: "消息".into(),
            role: BotNodeRole::Sink,
            binding: Some(outbound_binding(BOT_MEDIA_UPLOAD_V2_PROTOCOL_ID)),
            ports: vec![input_port(
                "input",
                "输入",
                "mutsuki.bot.media.upload",
                true,
            )],
            config_schema: json!({"type": "object", "additionalProperties": false}),
        },
    ]
}

fn outbound_binding(protocol_id: &str) -> BotNodeBinding {
    BotNodeBinding {
        binding_id: format!("binding:{protocol_id}"),
        protocol_id: protocol_id.into(),
        runner_hint: Some(BOT_FLOW_OUTBOUND_RUNNER_ID.into()),
    }
}

fn input_port(
    port_id: &str,
    title: &str,
    event_type: &str,
    required: bool,
) -> BotNodePortDescriptor {
    BotNodePortDescriptor {
        port_id: port_id.into(),
        title: title.into(),
        direction: BotNodePortDirection::Input,
        event_type: BotFlowTypeRef::new(event_type, 1),
        required,
    }
}

fn failure(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeFailure {
    RuntimeFailure::new(runtime_error(task, route, error))
}

fn runtime_error(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeError {
    mutsuki_bot_sdk::failure::runtime_error_with_message(
        BOT_FLOW_ROUTER_PLUGIN_ID,
        format!("flow.outbound.{route}.{}", task.task_id),
        error,
    )
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BotAccountRef, BotEvent, BotEventKind, BotExtMap, BotFlowEventEnvelope, BotMessage,
        BotNodeWiring, BotPlatform, BotTarget,
    };

    use super::*;

    fn invocation(config: Value) -> BotNodeInvocation {
        let event = BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::Sandbox,
            bot: BotAccountRef {
                account_id: "sandbox".into(),
                platform: BotPlatform::Sandbox,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "sandbox:default".into(),
            },
            actor: None,
            message: Some(BotMessage::text(
                BotTarget::Group {
                    group_id: "sandbox:default".into(),
                },
                "ping",
            )),
            raw: None,
            ext: BotExtMap::new(),
        };
        BotNodeInvocation {
            flow_id: "flow".into(),
            graph_revision: 1,
            execution_id: "exec".into(),
            node_id: "send".into(),
            input_port_id: "event".into(),
            wiring: BotNodeWiring::default(),
            config,
            input: BotFlowEventEnvelope::from_bot_event(event, None, None).unwrap(),
        }
    }

    #[test]
    fn send_node_routes_by_the_triggering_account_with_node_config_applied() {
        let request = node_request(
            BotOutboundOperation::Send,
            &invocation(json!({"text": "pong"})),
        )
        .unwrap();
        assert_eq!(request.account.platform, BotPlatform::Sandbox);
        assert_eq!(request.account.account_id, "sandbox");
        let message = BotMessage::deserialize(&request.request).unwrap();
        assert_eq!(message.plain_text(), "pong");
        assert_eq!(
            message.target,
            BotTarget::Group {
                group_id: "sandbox:default".into()
            }
        );
    }

    #[test]
    fn manifest_router_requires_no_endpoint_and_constructed_router_requires_the_plan_endpoints() {
        let declared = outbound_descriptor();
        assert!(
            declared
                .contract_surfaces
                .iter()
                .all(|surface| !surface.as_str().starts_with("requires:"))
        );
        let constructed = outbound_descriptor_for(&["mutsuki.bot.sandbox.message/send@1".into()]);
        assert!(
            constructed
                .contract_surfaces
                .iter()
                .any(|surface| surface.as_str()
                    == "requires:task_protocol:mutsuki.bot.sandbox.message/send@1")
        );
        for protocol in OUTBOUND_PROTOCOL_IDS {
            assert!(
                constructed
                    .accepted_protocol_ids
                    .iter()
                    .any(|accepted| accepted == protocol)
            );
        }
    }
}
