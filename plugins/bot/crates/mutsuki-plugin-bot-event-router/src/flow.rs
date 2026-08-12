use std::sync::Arc;

use mutsuki_bot_flow::{BotFlowError, BotFlowRegistry};
use mutsuki_bot_protocol::{
    BOT_FLOW_ERROR_TYPE, BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID,
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

use crate::{
    BOT_FLOW_EVENT_MATCH_PROTOCOL_ID, BOT_FLOW_MATCH_RUNNER_ID, BOT_FLOW_RATE_LIMIT_PROTOCOL_ID,
    match_descriptor, match_node_catalog,
};

pub const BOT_FLOW_ROUTER_PLUGIN_ID: &str = "mutsuki.bot.router.flow";
pub const BOT_FLOW_INGRESS_RUNNER_ID: &str = "mutsuki.bot.router.flow.ingress";
pub const BOT_FLOW_NODE_RUNNER_ID: &str = "mutsuki.bot.router.flow.node";
pub const BOT_FLOW_REGISTRY_SERVICE_ID: &str = "mutsuki.bot.flow.registry";

#[must_use]
pub fn flow_router_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_FLOW_ROUTER_PLUGIN_ID)
        .runner_descriptor(ingress_descriptor())
        .runner_descriptor(node_descriptor())
        .runner_descriptor(match_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_FLOW_INGRESS_PROTOCOL_ID).build(),
            BOT_FLOW_INGRESS_RUNNER_ID,
            "bot-flow-ingress",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID).build(),
            BOT_FLOW_NODE_RUNNER_ID,
            "bot-flow-node",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_FLOW_EVENT_MATCH_PROTOCOL_ID).build(),
            BOT_FLOW_MATCH_RUNNER_ID,
            "bot-flow-event-match",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_FLOW_RATE_LIMIT_PROTOCOL_ID).build(),
            BOT_FLOW_MATCH_RUNNER_ID,
            "bot-flow-rate-limit",
        )
        .extension(
            match_node_catalog()
                .into_plugin_extension()
                .expect("flow match node catalog serializes"),
        )
        .build()
        .manifest
}

pub fn flow_router_runners(
    client: RuntimeClientRef,
    registry: Arc<BotFlowRegistry>,
) -> Vec<Box<dyn Runner>> {
    let node_registry = registry.clone();
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let registry = node_registry.clone();
        Box::pin(run_node(ctx, task, registry))
    });
    vec![
        Box::new(BotFlowIngressRunner::new(registry)),
        Box::new(
            TaskAwaitRunnerAdapter::new(node_descriptor(), client, factory)
                .with_self_call_policy(false),
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
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let registry = registry.clone();
        Box::pin(run_node(ctx, task, registry))
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(node_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

pub struct BotFlowIngressRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    registry: Arc<BotFlowRegistry>,
}

impl BotFlowIngressRunner {
    pub fn new(registry: Arc<BotFlowRegistry>) -> Self {
        Self {
            descriptor: ingress_descriptor(),
            registry,
        }
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
        map_work_batch_entries(&batch, |task| {
            let envelope = task
                .payload
                .decode_shared::<BotFlowEventEnvelope>()
                .map_err(|error| runtime_error(task, "ingress.decode", error))?;
            let snapshot = self.registry.active();
            let mut tasks = Vec::new();
            for flow in snapshot.flows.iter().filter(|flow| flow.enabled) {
                for source in &flow.nodes {
                    let Some(selector) = source.source.as_ref() else {
                        continue;
                    };
                    if selector.protocol_id != envelope.protocol_id
                        || selector
                            .event_type
                            .as_ref()
                            .is_some_and(|event_type| event_type != &envelope.payload.event_type)
                    {
                        continue;
                    }
                    let execution_id = format!(
                        "flow:{}:{}:{}",
                        snapshot.revision, flow.flow_id, envelope.event_id
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
                                snapshot.revision,
                                &flow.flow_id,
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
            }
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.tasks = tasks;
            result.output = Some(serde_json::json!({
                "graph_revision": snapshot.revision,
                "flow_tasks": result.tasks.len(),
            }));
            Ok(result)
        })
    }
}

async fn run_node(
    ctx: AsyncRunnerContext,
    task: Task,
    registry: Arc<BotFlowRegistry>,
) -> RuntimeResult<RunnerResult> {
    let execution = task
        .payload
        .decode_shared::<BotFlowNodeExecution>()
        .map_err(|error| failure(&task, "node.decode", error))?;
    let snapshot = registry
        .published_revision(execution.graph_revision)
        .map_err(|error| flow_failure(&task, "node.snapshot", error))?;
    let flow = snapshot
        .flows
        .iter()
        .find(|flow| flow.flow_id == execution.flow_id)
        .ok_or_else(|| failure(&task, "node.flow_missing", &execution.flow_id))?;
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
        flow_id: execution.flow_id.clone(),
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

fn decode_node_result(task: &Task, outcome: TaskOutcome) -> RuntimeResult<BotNodeResult> {
    match outcome {
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
    execution: &BotFlowNodeExecution,
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
            &execution.flow_id,
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
    flow_id: &str,
    execution_id: &str,
    target: &BotFlowNode,
    edge: &BotFlowEdge,
    event: BotFlowEventEnvelope,
    registry_generation: u64,
    ordinal: usize,
) -> RuntimeResult<Task> {
    let execution = BotFlowNodeExecution {
        graph_revision: revision,
        flow_id: flow_id.into(),
        execution_id: execution_id.into(),
        node_id: target.node_id.clone(),
        input_port_id: edge.to_port_id.clone(),
        event,
    };
    let mut task = Task::new(
        format!(
            "{}:graph:{revision}:flow:{flow_id}:edge:{}:output:{ordinal}:node:{}",
            parent.task_id, edge.edge_id, target.node_id
        ),
        BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID,
        serde_json::to_value(execution)
            .map_err(|error| failure(parent, "node.execution.encode", error))?,
    );
    task.runner_hint = Some(BOT_FLOW_NODE_RUNNER_ID.into());
    task.trace_id = parent.trace_id.clone();
    task.correlation_id = parent.correlation_id.clone();
    task.idempotency_key = Some(task.task_id.clone());
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

fn node_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_FLOW_NODE_RUNNER_ID, BOT_FLOW_ROUTER_PLUGIN_ID)
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
        })
        .build()
}

fn flow_failure(task: &Task, route: &str, error: BotFlowError) -> RuntimeFailure {
    failure(task, route, error)
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
    use mutsuki_bot_protocol::{
        BotFlowContext, BotFlowEdge, BotFlowEdgeKind, BotFlowEventEnvelope, BotFlowNode,
        BotFlowNodePosition, BotFlowPayload, BotFlowTypeRef,
    };
    use mutsuki_runtime_contracts::{InvocationMode, RunnerConcurrency, Task};
    use serde_json::json;

    use super::{downstream_task, node_descriptor};

    #[test]
    fn cooperative_node_runner_dispatches_one_stateful_batch_without_blocking_waiters() {
        let descriptor = node_descriptor();

        assert_eq!(descriptor.invocation_mode, InvocationMode::SyncExclusive);
        assert_eq!(
            descriptor.concurrency,
            RunnerConcurrency::Sharded { instances: 1 }
        );
        assert_eq!(descriptor.batch.max_inflight_batches, 1);
    }

    #[test]
    fn downstream_task_ids_are_scoped_by_graph_and_flow() {
        let parent = Task::new("ingress", "mutsuki.bot.flow/ingress@1", json!({}));
        let target = BotFlowNode {
            node_id: "match".into(),
            node_type_id: "test.match".into(),
            node_type_version: 1,
            config: json!({}),
            source: None,
            position: BotFlowNodePosition::default(),
        };
        let edge = BotFlowEdge {
            edge_id: "source-match".into(),
            from_node_id: "source".into(),
            from_port_id: "event".into(),
            to_node_id: "match".into(),
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

        let left = downstream_task(
            &parent,
            1,
            "flow.left",
            "execution-left",
            &target,
            &edge,
            envelope.clone(),
            1,
            0,
        )
        .unwrap();
        let right = downstream_task(
            &parent,
            1,
            "flow.right",
            "execution-right",
            &target,
            &edge,
            envelope,
            1,
            0,
        )
        .unwrap();

        assert_ne!(left.task_id, right.task_id);
        assert!(left.task_id.contains("graph:1:flow:flow.left"));
        assert!(right.task_id.contains("graph:1:flow:flow.right"));
    }
}
