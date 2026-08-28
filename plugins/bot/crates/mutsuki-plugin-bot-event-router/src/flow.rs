use std::collections::HashMap;
use std::sync::Arc;

use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_protocol::{
    BOT_FLOW_ERROR_TYPE, BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID, BotEvent,
    BotFlowEdge, BotFlowEdgeKind, BotFlowErrorEvent, BotFlowEventEnvelope, BotFlowNode,
    BotFlowNodeExecution, BotFlowPayload, BotFlowTypeRef, BotNodeInvocation, BotNodeResult,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, InvocationMode, PluginManifest, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerMode, RunnerResult, RunnerSideEffect, RuntimeError, ScalarValue,
    Task, TaskOutcome, TimeoutGranularity,
};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, BoxedTaskAwaitRunner, PluginBuilder, ProtocolDescriptorBuilder,
    RunnerDescriptorBuilder, RuntimeClientRef, RuntimeFailure, RuntimeResult,
    TaskAwaitRunnerAdapter, map_work_batch_entries,
};
use serde::{Serialize, Serializer};
use serde_json::json;

use crate::{
    BOT_FLOW_MATCH_RUNNER_ID, MATCH_PROTOCOL_IDS, event_matches_source_types, match_descriptor,
    match_node_catalog, source_kinds_for_node,
};

pub const BOT_FLOW_ROUTER_PLUGIN_ID: &str = "mutsuki.bot.router.flow";
pub const BOT_FLOW_INGRESS_RUNNER_ID: &str = "mutsuki.bot.router.flow.ingress";
pub const BOT_FLOW_NODE_RUNNER_ID: &str = "mutsuki.bot.router.flow.node";
pub const BOT_FLOW_REGISTRY_SERVICE_ID: &str = "mutsuki.bot.flow.registry";

#[must_use]
pub fn flow_router_manifest() -> PluginManifest {
    flow_router_manifest_for_catalog(&[])
}

#[must_use]
pub fn flow_router_manifest_for_catalog(
    catalog: &[mutsuki_bot_protocol::BotNodeDescriptor],
) -> PluginManifest {
    let mut builder = PluginBuilder::new(BOT_FLOW_ROUTER_PLUGIN_ID)
        .runner_descriptor(ingress_descriptor())
        .runner_descriptor(node_descriptor(catalog))
        .runner_descriptor(match_descriptor())
        .protocol_handler(
            protocol_descriptor(
                BOT_FLOW_INGRESS_PROTOCOL_ID,
                &["event_id", "protocol_id", "payload", "context"],
                &["graph_revision", "flow_tasks"],
            ),
            BOT_FLOW_INGRESS_RUNNER_ID,
            "bot-flow-ingress",
        )
        .protocol_handler(
            protocol_descriptor(
                BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID,
                &["flow", "graph_revision", "execution_id", "node_id", "event"],
                &["output"],
            ),
            BOT_FLOW_NODE_RUNNER_ID,
            "bot-flow-node",
        );
    for protocol in MATCH_PROTOCOL_IDS {
        builder = builder.protocol_handler(
            protocol_descriptor(protocol, &["event"], &["matched"]),
            BOT_FLOW_MATCH_RUNNER_ID,
            (*protocol).to_string(),
        );
    }
    builder
        .extension(
            match_node_catalog()
                .into_plugin_extension()
                .expect("flow match node catalog serializes"),
        )
        .build()
        .manifest
}

fn protocol_descriptor(
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

pub fn flow_router_runners(
    client: RuntimeClientRef,
    registry: Arc<BotFlowRegistry>,
) -> Vec<Box<dyn Runner>> {
    let node_registry = registry.clone();
    let descriptor = node_descriptor(&node_registry.catalog());
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let registry = node_registry.clone();
        Box::pin(run_node(ctx, task, registry))
    });
    vec![
        Box::new(BotFlowIngressRunner::new(registry)),
        Box::new(
            TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false),
        ),
    ]
}

pub fn flow_ingress_runner(registry: Arc<BotFlowRegistry>) -> Box<dyn Runner> {
    Box::new(BotFlowIngressRunner::new(registry))
}

pub fn flow_node_runner(
    client: RuntimeClientRef,
    registry: Arc<BotFlowRegistry>,
) -> Box<dyn Runner> {
    let descriptor = node_descriptor(&registry.catalog());
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let registry = registry.clone();
        Box::pin(run_node(ctx, task, registry))
    });
    Box::new(TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false))
}

pub struct BotFlowIngressRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    registry: Arc<BotFlowRegistry>,
    source_index: Option<(u64, Arc<SourceIndex>)>,
}

struct SourceIndex {
    by_selector: HashMap<(String, Option<(String, u32)>), Vec<usize>>,
}

fn source_index_for(flow: &mutsuki_bot_protocol::BotFlowDocument) -> SourceIndex {
    let mut by_selector = HashMap::<(String, Option<(String, u32)>), Vec<usize>>::new();
    for (index, node) in flow.nodes.iter().enumerate() {
        let Some(selector) = node.source.as_ref() else {
            continue;
        };
        let event_type = selector
            .event_type
            .as_ref()
            .map(|event_type| (event_type.type_id.clone(), event_type.version));
        by_selector
            .entry((selector.protocol_id.clone(), event_type))
            .or_default()
            .push(index);
    }
    SourceIndex { by_selector }
}

/// Shares the immutable flow for native router tasks while preserving the
/// public `BotFlowNodeExecution` wire representation when a task crosses a
/// process boundary.
#[derive(Clone)]
struct PinnedBotFlowNodeExecution {
    graph_revision: u64,
    flow: Arc<mutsuki_bot_protocol::BotFlowDocument>,
    execution_id: String,
    node_id: String,
    input_port_id: String,
    event: BotFlowEventEnvelope,
}

impl Serialize for PinnedBotFlowNodeExecution {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BotFlowNodeExecution {
            graph_revision: self.graph_revision,
            flow: self.flow.as_ref().clone(),
            execution_id: self.execution_id.clone(),
            node_id: self.node_id.clone(),
            input_port_id: self.input_port_id.clone(),
            event: self.event.clone(),
        }
        .serialize(serializer)
    }
}

impl From<BotFlowNodeExecution> for PinnedBotFlowNodeExecution {
    fn from(execution: BotFlowNodeExecution) -> Self {
        Self {
            graph_revision: execution.graph_revision,
            flow: Arc::new(execution.flow),
            execution_id: execution.execution_id,
            node_id: execution.node_id,
            input_port_id: execution.input_port_id,
            event: execution.event,
        }
    }
}

impl BotFlowIngressRunner {
    pub fn new(registry: Arc<BotFlowRegistry>) -> Self {
        Self {
            descriptor: ingress_descriptor(),
            registry,
            source_index: None,
        }
    }

    fn source_index(
        &mut self,
        revision: u64,
        flow: &mutsuki_bot_protocol::BotFlowDocument,
    ) -> Arc<SourceIndex> {
        if let Some((cached_revision, index)) = &self.source_index
            && *cached_revision == revision
        {
            return index.clone();
        }
        let index = Arc::new(source_index_for(flow));
        self.source_index = Some((revision, index.clone()));
        index
    }
}

impl Runner for BotFlowIngressRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: mutsuki_runtime_core::RunnerContext,
        batch: mutsuki_runtime_contracts::WorkBatch,
    ) -> RuntimeResult<mutsuki_runtime_contracts::CompletionBatch> {
        let snapshot = self.registry.active();
        let graph_revision = snapshot.revision;
        let flow = Arc::new(snapshot.flow.clone());
        let index = self.source_index(graph_revision, flow.as_ref());
        map_work_batch_entries(&batch, |task| {
            let envelope = task
                .payload
                .decode_shared::<BotFlowEventEnvelope>()
                .map_err(|error| runtime_error(task, "ingress.decode", error))?;
            let event = serde_json::from_value::<BotEvent>(envelope.payload.value.clone()).ok();
            let mut source_indexes = Vec::new();
            if let Some(exact) = index.by_selector.get(&(
                envelope.protocol_id.clone(),
                Some((
                    envelope.payload.event_type.type_id.clone(),
                    envelope.payload.event_type.version,
                )),
            )) {
                source_indexes.extend(exact.iter().copied());
            }
            if let Some(wildcard) = index.by_selector.get(&(envelope.protocol_id.clone(), None)) {
                source_indexes.extend(wildcard.iter().copied());
            }
            source_indexes.sort_unstable();
            source_indexes.dedup();
            let mut tasks = Vec::new();
            for source_index in source_indexes {
                let source = &flow.nodes[source_index];
                if !source_accepts_event(source, event.as_ref()) {
                    continue;
                }
                let execution_id = format!(
                    "flow:{}:{}:{}",
                    graph_revision, flow.flow_id, envelope.event_id
                );
                let outgoing = flow.edges.iter().filter(|edge| {
                    edge.kind == BotFlowEdgeKind::Event && edge.from_node_id == source.node_id
                });
                for (ordinal, edge) in outgoing.enumerate() {
                    let Some(target) = flow
                        .nodes
                        .iter()
                        .find(|node| node.node_id == edge.to_node_id)
                    else {
                        continue;
                    };
                    tasks.push(
                        downstream_task(
                            task,
                            graph_revision,
                            flow.clone(),
                            &execution_id,
                            target,
                            edge,
                            envelope.as_ref().clone(),
                            ctx.registry_generation,
                            ordinal,
                        )
                        .map_err(|error| error.error().clone())?,
                    );
                }
            }
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.tasks = tasks;
            result.output = Some(serde_json::json!({
                "graph_revision": graph_revision,
                "flow_tasks": result.tasks.len(),
            }));
            Ok(result)
        })
    }
}

#[cfg(test)]
pub(crate) fn source_accepts_envelope(
    source: &BotFlowNode,
    envelope: &BotFlowEventEnvelope,
) -> bool {
    let event = serde_json::from_value::<BotEvent>(envelope.payload.value.clone()).ok();
    source_accepts_event(source, event.as_ref())
}

fn source_accepts_event(source: &BotFlowNode, event: Option<&BotEvent>) -> bool {
    let types = source_kinds_for_node(&source.node_type_id);
    match event {
        Some(event) if event.is_self_sent_message() => false,
        Some(event) => types.is_empty() || event_matches_source_types(event, types),
        None => types.is_empty(),
    }
}

async fn run_node(
    ctx: AsyncRunnerContext,
    task: Task,
    registry: Arc<BotFlowRegistry>,
) -> RuntimeResult<RunnerResult> {
    let execution = if let Some(execution) = task.payload.as_local::<PinnedBotFlowNodeExecution>() {
        execution
    } else {
        let wire = task
            .payload
            .decode_shared::<BotFlowNodeExecution>()
            .map_err(|error| failure(&task, "node.decode", error))?;
        Arc::new(PinnedBotFlowNodeExecution::from(wire.as_ref().clone()))
    };
    let flow = execution.flow.as_ref();
    let node = flow
        .nodes
        .iter()
        .find(|node| node.node_id == execution.node_id)
        .ok_or_else(|| failure(&task, "node.missing", &execution.node_id))?;
    let descriptor = registry
        .descriptor(&node.node_type_id, node.node_type_version)
        .ok_or_else(|| failure(&task, "node.type_unavailable", &node.node_type_id))?;
    let binding = descriptor
        .binding
        .as_ref()
        .ok_or_else(|| failure(&task, "node.binding_missing", &node.node_type_id))?;
    let invocation = BotNodeInvocation {
        flow_id: flow.flow_id.clone(),
        graph_revision: execution.graph_revision,
        execution_id: execution.execution_id.clone(),
        node_id: execution.node_id.clone(),
        input_port_id: execution.input_port_id.clone(),
        config: node.config.clone(),
        input: execution.event.clone(),
    };
    let payload = serde_json::to_value(invocation)
        .map_err(|error| failure(&task, "node.invocation.encode", error))?;
    let child = ctx
        .call_targeted_raw(
            binding.binding_id.clone(),
            binding.protocol_id.clone(),
            binding
                .runner_hint
                .clone()
                .unwrap_or_else(|| binding.binding_id.clone()),
            payload,
        )
        .await;
    let node_result = match child.and_then(|outcome| decode_node_result(&task, outcome)) {
        Ok(result) => result,
        Err(error) => {
            let error_edges = flow
                .edges
                .iter()
                .filter(|edge| {
                    edge.kind == BotFlowEdgeKind::Error && edge.from_node_id == node.node_id
                })
                .collect::<Vec<_>>();
            if error_edges.is_empty() {
                return Err(error);
            }
            let envelope = BotFlowEventEnvelope {
                event_id: format!("{}:error:{}", execution.event.event_id, node.node_id),
                protocol_id: "mutsuki.bot.flow/error@1".into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BOT_FLOW_ERROR_TYPE, 1),
                    value: serde_json::to_value(BotFlowErrorEvent {
                        failed_node_id: node.node_id.clone(),
                        error: error.error().clone(),
                        input: execution.event.clone(),
                    })
                    .map_err(|encode| failure(&task, "node.error.encode", encode))?,
                },
                context: execution.event.context.clone(),
                trace_id: execution.event.trace_id.clone(),
                correlation_id: execution.event.correlation_id.clone(),
            };
            return fan_out(
                &task,
                flow,
                &execution,
                error_edges.into_iter().map(|edge| (edge, envelope.clone())),
            );
        }
    };
    let outputs = node_result.outputs.iter().flat_map(|output| {
        flow.edges
            .iter()
            .filter(move |edge| {
                edge.kind == BotFlowEdgeKind::Event
                    && edge.from_node_id == node.node_id
                    && edge.from_port_id == output.port_id
            })
            .map(move |edge| (edge, output.event.clone()))
    });
    fan_out(&task, flow, &execution, outputs)
}

fn decode_node_result(
    task: &Task,
    outcome: impl Into<TaskOutcome>,
) -> RuntimeResult<BotNodeResult> {
    match outcome.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output)
            .map_err(|error| failure(task, "node.result.decode", error)),
        TaskOutcome::Completed { output: None, .. } => Ok(BotNodeResult {
            outputs: Vec::new(),
            metadata: Default::default(),
        }),
        TaskOutcome::Failed { error, .. } => Err(RuntimeFailure::new(error)),
        TaskOutcome::Cancelled { .. } => Err(failure(task, "node.child.cancelled", "cancelled")),
        TaskOutcome::Expired { .. } => Err(failure(task, "node.child.expired", "expired")),
        TaskOutcome::DeadLetter { .. } => {
            Err(failure(task, "node.child.dead_letter", "dead letter"))
        }
    }
}

fn fan_out<'a>(
    task: &Task,
    flow: &mutsuki_bot_protocol::BotFlowDocument,
    execution: &PinnedBotFlowNodeExecution,
    outputs: impl IntoIterator<Item = (&'a BotFlowEdge, BotFlowEventEnvelope)>,
) -> RuntimeResult<RunnerResult> {
    let mut result = RunnerResult::completed(task.task_id.clone());
    for (ordinal, (edge, envelope)) in outputs.into_iter().enumerate() {
        let target = flow
            .nodes
            .iter()
            .find(|candidate| candidate.node_id == edge.to_node_id)
            .ok_or_else(|| failure(task, "node.edge_target_missing", &edge.to_node_id))?;
        result.tasks.push(downstream_task(
            task,
            execution.graph_revision,
            execution.flow.clone(),
            &execution.execution_id,
            target,
            edge,
            envelope,
            task.registry_generation,
            ordinal,
        )?);
    }
    result.output = Some(serde_json::json!({"fanout": result.tasks.len()}));
    Ok(result)
}

fn downstream_task(
    parent: &Task,
    revision: u64,
    flow: Arc<mutsuki_bot_protocol::BotFlowDocument>,
    execution_id: &str,
    target: &BotFlowNode,
    edge: &BotFlowEdge,
    event: BotFlowEventEnvelope,
    registry_generation: u64,
    ordinal: usize,
) -> RuntimeResult<Task> {
    let execution = PinnedBotFlowNodeExecution {
        graph_revision: revision,
        flow,
        execution_id: execution_id.into(),
        node_id: target.node_id.clone(),
        input_port_id: edge.to_port_id.clone(),
        event,
    };
    let mut task = Task::new(
        format!(
            "{}:graph:{revision}:flow:{}:edge:{}:output:{ordinal}:node:{}",
            parent.task_id, execution.flow.flow_id, edge.edge_id, target.node_id
        ),
        BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID,
        mutsuki_runtime_contracts::TaskPayload::from_local(execution),
    );
    task.runner_hint = Some(BOT_FLOW_NODE_RUNNER_ID.into());
    task.trace_id = parent.trace_id.clone();
    task.correlation_id = parent.correlation_id.clone();
    task.idempotency_key = Some(task.task_id.to_string());
    task.registry_generation = registry_generation;
    Ok(task)
}

fn ingress_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_FLOW_INGRESS_RUNNER_ID, BOT_FLOW_ROUTER_PLUGIN_ID)
        .accepted_protocol(BOT_FLOW_INGRESS_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .invocation_mode(InvocationMode::SyncExclusive)
        .concurrency(RunnerConcurrency::Exclusive)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 32,
            max_batch_entries: 128,
            max_entry_concurrency: 1,
            max_inflight_batches: 1,
            side_effect: RunnerSideEffect::None,
            ..RunnerBatchCapability::default()
        })
        .build()
}

fn node_descriptor(
    catalog: &[mutsuki_bot_protocol::BotNodeDescriptor],
) -> mutsuki_runtime_contracts::RunnerDescriptor {
    let builder = RunnerDescriptorBuilder::new(BOT_FLOW_NODE_RUNNER_ID, BOT_FLOW_ROUTER_PLUGIN_ID)
        .accepted_protocol(BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID)
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
        });
    catalog
        .iter()
        .filter_map(|descriptor| descriptor.binding.as_ref())
        .fold(builder, |builder, binding| {
            builder.requires_protocol(binding.protocol_id.clone())
        })
        .build()
}

fn failure(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeFailure {
    RuntimeFailure::new(runtime_error(task, route, error))
}

fn runtime_error(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeError {
    let mut error_value = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_FLOW_ROUTER_PLUGIN_ID,
        format!("flow.{route}.{}", task.task_id),
    );
    error_value
        .evidence
        .insert("message".into(), ScalarValue::String(error.to_string()));
    error_value
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mutsuki_bot_protocol::{
        BOT_SELF_SENT_EXT_KEY, BotAccountRef, BotEvent, BotEventKind, BotFlowContext, BotFlowEdge,
        BotFlowEdgeKind, BotFlowEventEnvelope, BotFlowNode, BotFlowNodePosition, BotFlowPayload,
        BotFlowSourceSelector, BotFlowTypeRef, BotPlatform, BotTarget, BotUser,
    };
    use mutsuki_runtime_contracts::{InvocationMode, RunnerConcurrency, Task};
    use serde_json::{Value, json};

    use super::{downstream_task, node_descriptor, source_accepts_envelope, source_index_for};

    fn message_envelope(actor_id: &str, self_sent: bool) -> BotFlowEventEnvelope {
        let mut ext = mutsuki_bot_protocol::BotExtMap::new();
        if self_sent {
            ext.insert(BOT_SELF_SENT_EXT_KEY.into(), Value::Bool(true));
        }
        let event = BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "qq-main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: Some(BotUser {
                user_id: actor_id.into(),
                display_name: Some("n".into()),
                avatar_url: None,
            }),
            message: None,
            raw: None,
            ext,
        };
        BotFlowEventEnvelope {
            event_id: "e1".into(),
            protocol_id: "mutsuki.bot.event/ingest@1".into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
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
        }
    }

    #[test]
    fn source_index_selects_exact_and_protocol_wildcard_without_unrelated_nodes() {
        let ingest = mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID;
        let command = mutsuki_bot_protocol::BOT_COMMAND_HANDLE_PROTOCOL_ID;
        let flow = mutsuki_bot_protocol::BotFlowDocument {
            flow_id: "indexed".into(),
            name: "indexed".into(),
            nodes: vec![
                BotFlowNode {
                    node_id: "exact".into(),
                    node_type_id: "mutsuki.bot.qq.message.created".into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: Some(BotFlowSourceSelector {
                        protocol_id: ingest.into(),
                        event_type: Some(BotFlowTypeRef::new("mutsuki.bot.event", 1)),
                    }),
                    position: BotFlowNodePosition::default(),
                },
                BotFlowNode {
                    node_id: "wildcard".into(),
                    node_type_id: "mutsuki.bot.qq.message.created".into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: Some(BotFlowSourceSelector {
                        protocol_id: ingest.into(),
                        event_type: None,
                    }),
                    position: BotFlowNodePosition::default(),
                },
                BotFlowNode {
                    node_id: "command".into(),
                    node_type_id: "mutsuki.bot.command.parse".into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: Some(BotFlowSourceSelector {
                        protocol_id: command.into(),
                        event_type: None,
                    }),
                    position: BotFlowNodePosition::default(),
                },
            ],
            edges: vec![],
        };
        let index = source_index_for(&flow);
        let exact = index
            .by_selector
            .get(&(ingest.into(), Some(("mutsuki.bot.event".into(), 1))))
            .cloned()
            .unwrap_or_default();
        let wildcard = index
            .by_selector
            .get(&(ingest.into(), None))
            .cloned()
            .unwrap_or_default();
        let command_nodes = index
            .by_selector
            .get(&(command.into(), None))
            .cloned()
            .unwrap_or_default();
        assert_eq!(exact, vec![0]);
        assert_eq!(wildcard, vec![1]);
        assert_eq!(command_nodes, vec![2]);
    }

    #[test]
    fn ingress_rejects_bot_self_sent_messages() {
        let source = BotFlowNode {
            node_id: "qq-message".into(),
            node_type_id: "mutsuki.bot.qq.message.created".into(),
            node_type_version: 1,
            config: json!({}),
            source: Some(BotFlowSourceSelector {
                protocol_id: "mutsuki.bot.event/ingest@1".into(),
                event_type: None,
            }),
            position: BotFlowNodePosition::default(),
        };
        assert!(source_accepts_envelope(
            &source,
            &message_envelope("member-1", false)
        ));
        assert!(!source_accepts_envelope(
            &source,
            &message_envelope("BOT_OPENID", true)
        ));
    }

    #[test]
    fn cooperative_node_runner_dispatches_one_stateful_batch_without_blocking_waiters() {
        let descriptor = node_descriptor(&[]);

        assert_eq!(descriptor.invocation_mode, InvocationMode::SyncExclusive);
        assert_eq!(
            descriptor.concurrency,
            RunnerConcurrency::Sharded { instances: 1 }
        );
        assert_eq!(descriptor.batch.max_inflight_batches, 1);
    }

    #[test]
    fn downstream_task_ids_are_scoped_by_graph_and_edge() {
        let parent = Task::new("ingress", "mutsuki.bot.flow/ingress@1", json!({}));
        let left_target = BotFlowNode {
            node_id: "match-left".into(),
            node_type_id: "test.match".into(),
            node_type_version: 1,
            config: json!({}),
            source: None,
            position: BotFlowNodePosition::default(),
        };
        let right_target = BotFlowNode {
            node_id: "match-right".into(),
            node_type_id: "test.match".into(),
            node_type_version: 1,
            config: json!({}),
            source: None,
            position: BotFlowNodePosition::default(),
        };
        let left_edge = BotFlowEdge {
            edge_id: "source-left".into(),
            from_node_id: "source".into(),
            from_port_id: "event".into(),
            to_node_id: "match-left".into(),
            to_port_id: "event".into(),
            kind: BotFlowEdgeKind::Event,
        };
        let right_edge = BotFlowEdge {
            edge_id: "source-right".into(),
            from_node_id: "source".into(),
            from_port_id: "event".into(),
            to_node_id: "match-right".into(),
            to_port_id: "event".into(),
            kind: BotFlowEdgeKind::Event,
        };
        let envelope = BotFlowEventEnvelope {
            event_id: "event".into(),
            protocol_id: "mutsuki.bot.event/ingest@1".into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
                value: json!({}),
            },
            context: BotFlowContext {
                bot: None,
                target: None,
                actor: None,
                ext: Default::default(),
            },
            trace_id: None,
            correlation_id: None,
        };
        let flow = mutsuki_bot_protocol::BotFlowDocument {
            flow_id: "default".into(),
            name: "流程".into(),
            nodes: vec![left_target.clone(), right_target.clone()],
            edges: vec![left_edge.clone(), right_edge.clone()],
        };

        let left = downstream_task(
            &parent,
            1,
            Arc::new(flow.clone()),
            "execution",
            &left_target,
            &left_edge,
            envelope.clone(),
            1,
            0,
        )
        .unwrap();
        let right = downstream_task(
            &parent,
            1,
            Arc::new(flow),
            "execution",
            &right_target,
            &right_edge,
            envelope,
            1,
            0,
        )
        .unwrap();

        assert_ne!(left.task_id, right.task_id);
        assert!(
            left.task_id
                .as_str()
                .contains("graph:1:flow:default:edge:source-left")
        );
        assert!(
            right
                .task_id
                .as_str()
                .contains("graph:1:flow:default:edge:source-right")
        );
    }
}
