use std::collections::HashMap;
use std::sync::Arc;

use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_protocol::{
    BOT_FLOW_ERROR_TYPE, BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID, BotEvent,
    BotFlowDocument, BotFlowEdge, BotFlowEdgeKind, BotFlowErrorEvent, BotFlowEventEnvelope,
    BotFlowNode, BotFlowNodeExecution, BotFlowPayload, BotFlowTypeRef, BotNodeInvocation,
    BotNodeResult, BotNodeWiring,
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
use serde::{Deserialize, Serialize, Serializer};
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
    graph_index: Option<(u64, Arc<GraphIndex>)>,
}

/// Source nodes of one ingress protocol, split so a lookup borrows its key.
#[derive(Default)]
struct ProtocolSources {
    /// Sources that accept every event type on the protocol.
    wildcard: Vec<usize>,
    by_type: HashMap<String, HashMap<u32, Vec<usize>>>,
}

type SelectorIndex = HashMap<String, ProtocolSources>;

/// Everything the router derives from one immutable graph revision.
///
/// The document is pinned for the lifetime of an execution, so node lookup,
/// outgoing edges and port wiring are all fixed the moment a revision is applied.
/// Recomputing them per hop cost a linear scan of `nodes` and `edges` plus fresh
/// allocations on every single event; `node_wiring` alone walked every edge and
/// sorted two vectors. Building this once per revision turns each of those into a
/// hash lookup, and lets every hop of an execution share one `Arc` of the document
/// instead of deep-cloning it per batch.
struct GraphIndex {
    flow: Arc<BotFlowDocument>,
    node_by_id: HashMap<String, usize>,
    /// Indices into `flow.edges`, in document order so fan-out ordinals are stable.
    event_edges_from: HashMap<String, Vec<usize>>,
    error_edges_from: HashMap<String, Vec<usize>>,
    wiring_by_node: HashMap<String, BotNodeWiring>,
    by_selector: SelectorIndex,
}

impl GraphIndex {
    fn new(flow: Arc<BotFlowDocument>) -> Self {
        let mut node_by_id = HashMap::with_capacity(flow.nodes.len());
        let mut by_selector = SelectorIndex::new();
        for (index, node) in flow.nodes.iter().enumerate() {
            node_by_id.insert(node.node_id.clone(), index);
            let Some(selector) = node.source.as_ref() else {
                continue;
            };
            let sources = by_selector.entry(selector.protocol_id.clone()).or_default();
            match selector.event_type.as_ref() {
                Some(event_type) => sources
                    .by_type
                    .entry(event_type.type_id.clone())
                    .or_default()
                    .entry(event_type.version)
                    .or_default()
                    .push(index),
                None => sources.wildcard.push(index),
            }
        }

        let mut event_edges_from: HashMap<String, Vec<usize>> = HashMap::new();
        let mut error_edges_from: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, edge) in flow.edges.iter().enumerate() {
            let bucket = match edge.kind {
                BotFlowEdgeKind::Event => &mut event_edges_from,
                BotFlowEdgeKind::Error => &mut error_edges_from,
            };
            bucket
                .entry(edge.from_node_id.clone())
                .or_default()
                .push(index);
        }

        let wiring_by_node = flow
            .nodes
            .iter()
            .map(|node| {
                (
                    node.node_id.clone(),
                    mutsuki_bot_flow::node_wiring(&flow, &node.node_id).unwrap_or_default(),
                )
            })
            .collect();

        Self {
            flow,
            node_by_id,
            event_edges_from,
            error_edges_from,
            wiring_by_node,
            by_selector,
        }
    }

    fn node(&self, node_id: &str) -> Option<&BotFlowNode> {
        self.node_by_id
            .get(node_id)
            .map(|index| &self.flow.nodes[*index])
    }

    fn event_edges(&self, from_node_id: &str) -> impl Iterator<Item = &BotFlowEdge> {
        self.event_edges_from
            .get(from_node_id)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|index| &self.flow.edges[*index])
    }

    fn error_edges(&self, from_node_id: &str) -> impl Iterator<Item = &BotFlowEdge> {
        self.error_edges_from
            .get(from_node_id)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|index| &self.flow.edges[*index])
    }

    fn wiring(&self, node_id: &str) -> BotNodeWiring {
        self.wiring_by_node
            .get(node_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Source node indices matching one ingress selector, exact types first.
    fn sources_for(&self, protocol_id: &str, event_type: (&str, u32)) -> Vec<usize> {
        let Some(sources) = self.by_selector.get(protocol_id) else {
            return Vec::new();
        };
        let exact = sources
            .by_type
            .get(event_type.0)
            .and_then(|versions| versions.get(&event_type.1))
            .map_or(&[][..], Vec::as_slice);
        let mut matched = Vec::with_capacity(exact.len() + sources.wildcard.len());
        matched.extend_from_slice(exact);
        matched.extend_from_slice(&sources.wildcard);
        matched.sort_unstable();
        matched.dedup();
        matched
    }
}

/// Shares the immutable flow for native router tasks while preserving the
/// public `BotFlowNodeExecution` wire representation when a task crosses a
/// process boundary.
#[derive(Clone)]
struct PinnedBotFlowNodeExecution {
    graph_revision: u64,
    index: Arc<GraphIndex>,
    execution_id: String,
    node_id: String,
    input_port_id: String,
    /// Shared rather than owned: one node's output fans out to every wired edge,
    /// and the envelope carries the whole event payload.
    event: Arc<BotFlowEventEnvelope>,
}

impl Serialize for PinnedBotFlowNodeExecution {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BotFlowNodeExecution {
            graph_revision: self.graph_revision,
            flow: self.index.flow.as_ref().clone(),
            execution_id: self.execution_id.clone(),
            node_id: self.node_id.clone(),
            input_port_id: self.input_port_id.clone(),
            event: self.event.as_ref().clone(),
        }
        .serialize(serializer)
    }
}

impl From<BotFlowNodeExecution> for PinnedBotFlowNodeExecution {
    fn from(execution: BotFlowNodeExecution) -> Self {
        Self {
            graph_revision: execution.graph_revision,
            // Only reached when a task crossed a process boundary: the in-process
            // path shares the index built once for the revision.
            index: Arc::new(GraphIndex::new(Arc::new(execution.flow))),
            execution_id: execution.execution_id,
            node_id: execution.node_id,
            input_port_id: execution.input_port_id,
            event: Arc::new(execution.event),
        }
    }
}

impl BotFlowIngressRunner {
    pub fn new(registry: Arc<BotFlowRegistry>) -> Self {
        Self {
            descriptor: ingress_descriptor(),
            registry,
            graph_index: None,
        }
    }

    /// The derived index for the active revision, rebuilt only when it changes.
    ///
    /// The document is cloned out of the snapshot once per revision rather than
    /// once per batch; every hop of every execution then shares that one `Arc`.
    fn graph_index(&mut self, snapshot: &mutsuki_bot_protocol::BotFlowSnapshot) -> Arc<GraphIndex> {
        if let Some((cached_revision, index)) = &self.graph_index
            && *cached_revision == snapshot.revision
        {
            return index.clone();
        }
        let index = Arc::new(GraphIndex::new(Arc::new(snapshot.flow.clone())));
        self.graph_index = Some((snapshot.revision, index.clone()));
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
        let index = self.graph_index(&snapshot);
        let stats = self.registry.ingress_stats();
        map_work_batch_entries(&batch, |task| {
            let envelope = task
                .payload
                .decode_shared::<BotFlowEventEnvelope>()
                .map_err(|error| runtime_error(task, "ingress.decode", error))?;
            // Borrowed rather than cloned: the payload is the whole inbound event.
            let event = BotEvent::deserialize(&envelope.payload.value).ok();
            let source_indexes = index.sources_for(
                &envelope.protocol_id,
                (
                    &envelope.payload.event_type.type_id,
                    envelope.payload.event_type.version,
                ),
            );
            // Invariant across sources and edges, so it is built once per event.
            let execution_id = format!(
                "flow:{}:{}:{}",
                graph_revision, index.flow.flow_id, envelope.event_id
            );
            let mut matched_sources = 0_usize;
            let mut tasks = Vec::new();
            for source_index in source_indexes {
                let source = &index.flow.nodes[source_index];
                if !source_accepts_event(source, event.as_ref()) {
                    continue;
                }
                matched_sources += 1;
                for (ordinal, edge) in index.event_edges(&source.node_id).enumerate() {
                    let Some(target) = index.node(&edge.to_node_id) else {
                        continue;
                    };
                    tasks.push(
                        downstream_task(
                            task,
                            graph_revision,
                            index.clone(),
                            &execution_id,
                            target,
                            edge,
                            envelope.clone(),
                            ctx.registry_generation,
                            ordinal,
                        )
                        .map_err(|error| error.error().clone())?,
                    );
                }
            }
            record_ingress_outcome(stats, event.as_ref(), matched_sources);
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.tasks = tasks;
            result.output = Some(serde_json::json!({
                "graph_revision": graph_revision,
                "flow_tasks": result.tasks.len(),
                "matched_sources": matched_sources,
            }));
            Ok(result)
        })
    }
}

/// Bot self-sent projections never start Source chains by design, so they are
/// not business events and stay out of the frozen-traffic counters.
fn record_ingress_outcome(
    stats: &mutsuki_bot_flow::BotFlowIngressStats,
    event: Option<&BotEvent>,
    matched_sources: usize,
) {
    if event.is_some_and(BotEvent::is_self_sent_message) {
        return;
    }
    stats.record_accepted();
    if matched_sources == 0 {
        stats.record_dropped();
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
    let index = execution.index.clone();
    let node = index
        .node(&execution.node_id)
        .ok_or_else(|| failure(&task, "node.missing", &execution.node_id))?;
    let descriptor = registry
        .descriptor(&node.node_type_id, node.node_type_version)
        .ok_or_else(|| failure(&task, "node.type_unavailable", &node.node_type_id))?;
    let binding = descriptor
        .binding
        .as_ref()
        .ok_or_else(|| failure(&task, "node.binding_missing", &node.node_type_id))?;
    let invocation = node_invocation(&index, &execution, node);
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
            let error_edges = index.error_edges(&node.node_id).collect::<Vec<_>>();
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
                        input: execution.event.as_ref().clone(),
                    })
                    .map_err(|encode| failure(&task, "node.error.encode", encode))?,
                },
                context: execution.event.context.clone(),
                trace_id: execution.event.trace_id.clone(),
                correlation_id: execution.event.correlation_id.clone(),
            };
            let envelope = Arc::new(envelope);
            return fan_out(
                &task,
                &index,
                &execution,
                error_edges.into_iter().map(|edge| (edge, envelope.clone())),
            );
        }
    };
    // One Arc per node output, shared by every edge wired to that port.
    let outputs = node_result
        .outputs
        .iter()
        .map(|output| (output, Arc::new(output.event.clone())))
        .collect::<Vec<_>>();
    let outputs = outputs.iter().flat_map(|(output, event)| {
        index
            .event_edges(&node.node_id)
            .filter(move |edge| edge.from_port_id == output.port_id)
            .map(move |edge| (edge, event.clone()))
    });
    fan_out(&task, &index, &execution, outputs)
}

/// Builds the plugin-facing invocation for one node execution. The wiring is
/// derived from the pinned immutable graph so the node learns whether it is
/// connected without holding the document itself.
fn node_invocation(
    index: &GraphIndex,
    execution: &PinnedBotFlowNodeExecution,
    node: &BotFlowNode,
) -> BotNodeInvocation {
    BotNodeInvocation {
        flow_id: index.flow.flow_id.clone(),
        graph_revision: execution.graph_revision,
        execution_id: execution.execution_id.clone(),
        node_id: execution.node_id.clone(),
        input_port_id: execution.input_port_id.clone(),
        wiring: index.wiring(&execution.node_id),
        config: node.config.clone(),
        input: execution.event.as_ref().clone(),
    }
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
    index: &Arc<GraphIndex>,
    execution: &PinnedBotFlowNodeExecution,
    outputs: impl IntoIterator<Item = (&'a BotFlowEdge, Arc<BotFlowEventEnvelope>)>,
) -> RuntimeResult<RunnerResult> {
    let mut result = RunnerResult::completed(task.task_id.clone());
    for (ordinal, (edge, envelope)) in outputs.into_iter().enumerate() {
        let target = index
            .node(&edge.to_node_id)
            .ok_or_else(|| failure(task, "node.edge_target_missing", &edge.to_node_id))?;
        result.tasks.push(downstream_task(
            task,
            execution.graph_revision,
            index.clone(),
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
    index: Arc<GraphIndex>,
    execution_id: &str,
    target: &BotFlowNode,
    edge: &BotFlowEdge,
    event: Arc<BotFlowEventEnvelope>,
    registry_generation: u64,
    ordinal: usize,
) -> RuntimeResult<Task> {
    let execution = PinnedBotFlowNodeExecution {
        graph_revision: revision,
        index,
        execution_id: execution_id.into(),
        node_id: target.node_id.clone(),
        input_port_id: edge.to_port_id.clone(),
        event,
    };
    let mut task = Task::new(
        format!(
            "{}:graph:{revision}:flow:{}:edge:{}:output:{ordinal}:node:{}",
            parent.task_id, execution.index.flow.flow_id, edge.edge_id, target.node_id
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
        BotFlowEdgeKind, BotFlowEventEnvelope, BotFlowNode, BotFlowNodeExecution,
        BotFlowNodePosition, BotFlowPayload, BotFlowSourceSelector, BotFlowTypeRef, BotPlatform,
        BotTarget, BotUser,
    };
    use mutsuki_runtime_contracts::{InvocationMode, RunnerConcurrency, Task};
    use serde_json::{Value, json};

    use super::{
        GraphIndex, PinnedBotFlowNodeExecution, downstream_task, node_descriptor, node_invocation,
        record_ingress_outcome, source_accepts_envelope,
    };

    fn message_event(actor_id: &str, self_sent: bool) -> BotEvent {
        let mut ext = mutsuki_bot_protocol::BotExtMap::new();
        if self_sent {
            ext.insert(BOT_SELF_SENT_EXT_KEY.into(), Value::Bool(true));
        }
        BotEvent {
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
        }
    }

    fn message_envelope(actor_id: &str, self_sent: bool) -> BotFlowEventEnvelope {
        let event = message_event(actor_id, self_sent);
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
        let index = GraphIndex::new(Arc::new(flow));
        // An exactly-typed source and a wildcard source on the same protocol both
        // match, merged and deduped in node order.
        assert_eq!(
            index.sources_for(ingest, ("mutsuki.bot.event", 1)),
            vec![0, 1]
        );
        // A different event type on that protocol leaves only the wildcard.
        assert_eq!(index.sources_for(ingest, ("other.event", 1)), vec![1]);
        // A version mismatch is not an exact match either.
        assert_eq!(index.sources_for(ingest, ("mutsuki.bot.event", 2)), vec![1]);
        assert_eq!(index.sources_for(command, ("anything", 1)), vec![2]);
        assert!(index.sources_for("no.such.protocol", ("x", 1)).is_empty());
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
    fn plugin_typed_event_sources_match_by_selector_without_a_kind_entry() {
        let ingest = mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID;
        let flow = mutsuki_bot_protocol::BotFlowDocument {
            flow_id: "push".into(),
            name: "push".into(),
            nodes: vec![BotFlowNode {
                node_id: "bili".into(),
                node_type_id: "mutsuki.bot.bilibili.notification".into(),
                node_type_version: 1,
                config: json!({}),
                source: Some(BotFlowSourceSelector {
                    protocol_id: ingest.into(),
                    event_type: Some(BotFlowTypeRef::new("mutsuki.bot.event.bilibili", 1)),
                }),
                position: BotFlowNodePosition::default(),
            }],
            edges: vec![],
        };
        let index = GraphIndex::new(Arc::new(flow.clone()));
        assert_eq!(
            index.sources_for(ingest, ("mutsuki.bot.event.bilibili", 1)),
            vec![0]
        );
        assert!(
            index
                .sources_for(ingest, ("mutsuki.bot.event", 1))
                .is_empty(),
            "a typed source must not answer a different event type"
        );

        let envelope = BotFlowEventEnvelope {
            event_id: "notify-1".into(),
            protocol_id: ingest.into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new("mutsuki.bot.event.bilibili", 1),
                value: json!({"kind": "live", "uid": 42}),
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
        assert!(source_accepts_envelope(&flow.nodes[0], &envelope));

        let qq_source = BotFlowNode {
            node_id: "qq-message".into(),
            node_type_id: "mutsuki.bot.qq.message.created".into(),
            node_type_version: 1,
            config: json!({}),
            source: Some(BotFlowSourceSelector {
                protocol_id: ingest.into(),
                event_type: None,
            }),
            position: BotFlowNodePosition::default(),
        };
        assert!(!source_accepts_envelope(&qq_source, &envelope));
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
    fn ingress_stats_count_unmatched_business_events_and_skip_self_sent() {
        let registry = Arc::new(mutsuki_bot_flow::BotFlowRegistry::new(
            mutsuki_bot_flow::BotNodeCatalog::default(),
        ));
        let stats = registry.ingress_stats();

        record_ingress_outcome(stats, Some(&message_event("member-1", false)), 0);
        assert_eq!(stats.accepted_total(), 1);
        assert_eq!(stats.dropped_total(), 1);

        record_ingress_outcome(stats, Some(&message_event("member-2", false)), 2);
        assert_eq!(stats.accepted_total(), 2);
        assert_eq!(stats.dropped_total(), 1);

        record_ingress_outcome(stats, Some(&message_event("BOT_OPENID", true)), 0);
        assert_eq!(stats.accepted_total(), 2);
        assert_eq!(stats.dropped_total(), 1);

        // A non-BotEvent payload (plugin-owned typed event) is a business event.
        record_ingress_outcome(stats, None, 0);
        assert_eq!(stats.accepted_total(), 3);
        assert_eq!(stats.dropped_total(), 2);
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
            Arc::new(GraphIndex::new(Arc::new(flow.clone()))),
            "execution",
            &left_target,
            &left_edge,
            Arc::new(envelope.clone()),
            1,
            0,
        )
        .unwrap();
        let right = downstream_task(
            &parent,
            1,
            Arc::new(GraphIndex::new(Arc::new(flow))),
            "execution",
            &right_target,
            &right_edge,
            Arc::new(envelope),
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

    #[test]
    fn node_invocation_carries_port_level_wiring_from_the_pinned_graph() {
        let flow = mutsuki_bot_protocol::BotFlowDocument {
            flow_id: "wired".into(),
            name: "wired".into(),
            nodes: vec![
                BotFlowNode {
                    node_id: "source".into(),
                    node_type_id: "test.source".into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: Some(BotFlowSourceSelector {
                        protocol_id: "test.ingress".into(),
                        event_type: None,
                    }),
                    position: BotFlowNodePosition::default(),
                },
                BotFlowNode {
                    node_id: "downstream".into(),
                    node_type_id: "test.process".into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: None,
                    position: BotFlowNodePosition::default(),
                },
            ],
            edges: vec![BotFlowEdge {
                edge_id: "source-downstream".into(),
                from_node_id: "source".into(),
                from_port_id: "event".into(),
                to_node_id: "downstream".into(),
                to_port_id: "input".into(),
                kind: BotFlowEdgeKind::Event,
            }],
        };
        let execution_for = |node_id: &str| {
            PinnedBotFlowNodeExecution::from(BotFlowNodeExecution {
                graph_revision: 3,
                flow: flow.clone(),
                execution_id: "exec".into(),
                node_id: node_id.into(),
                input_port_id: "input".into(),
                event: message_envelope("member-1", false),
            })
        };
        let source_node = flow.nodes[0].clone();
        let downstream_node = flow.nodes[1].clone();
        let index = GraphIndex::new(Arc::new(flow.clone()));

        let source = node_invocation(&index, &execution_for("source"), &source_node);
        assert_eq!(source.wiring.wired_outputs, vec!["event".to_owned()]);
        assert!(source.wiring.wired_inputs.is_empty());
        assert!(source.wiring.is_connected());

        let downstream = node_invocation(&index, &execution_for("downstream"), &downstream_node);
        assert_eq!(downstream.wiring.wired_inputs, vec!["input".to_owned()]);
        assert!(downstream.wiring.wired_outputs.is_empty());
        assert!(downstream.wiring.is_connected());
        assert!(!downstream.wiring.has_downstream());
    }
}
