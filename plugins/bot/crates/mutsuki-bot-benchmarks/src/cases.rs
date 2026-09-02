use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use async_trait::async_trait;
use mutsuki_bot_conversation::{ConversationError, ConversationRepository, ConversationService};
use mutsuki_bot_delivery::{
    ActiveDeliveryService, DeliveryError, DeliveryFailure, DeliveryGateway, DeliveryPolicyResolver,
    DeliveryRepository, DeliverySuccess,
};
use mutsuki_bot_flow::{BotFlowRegistry, BotNodeCatalog};
use mutsuki_bot_interaction::{
    InteractionConditionMatcher, InteractionError, InteractionRepository, InteractionService,
};
use mutsuki_bot_link_parser::{expand_card_payload, extract_urls};
use mutsuki_bot_protocol::*;
use mutsuki_bot_sandbox::{SandboxApi, SandboxService};
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_bot_testkit::{
    BENCHMARK_FIXED_SEED, benchmark_card_payload, benchmark_event, benchmark_gateway_frame,
};
use mutsuki_plugin_bot_adapter_qqbot::tasks::qqbot_adapter_manifest;
use mutsuki_plugin_bot_adapter_qqbot::{
    GatewayAction, GatewayFrame, HttpMethod, QQBOT_GATEWAY_FRAME_PROTOCOL_ID, QqBotConfig,
    QqGatewayMapRunner, QqGatewayPump, QqHttpClient, QqHttpRequest, QqHttpResponse, QqOpenApiError,
    QqOpenApiTransport, StaticQqCredentials,
};
use mutsuki_plugin_bot_command::{
    BOT_COMMAND_MATCH_NODE_TYPE_ID, BOT_COMMAND_RUNNER_ID, BotCommandNodeRunner,
    bot_command_manifest,
};
use mutsuki_plugin_bot_event_router::{
    BOT_FLOW_INGRESS_RUNNER_ID, BotFlowIngressRunner, flow_router_manifest,
};
use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CompletionBatch, DispatchLane, OrderingRequirement, RunnerContext,
    RunnerResult, RunnerStatus, RuntimeError, Task, TaskBatch, TaskHandle, TaskId, TaskOutcome,
    TaskPayload, WorkBatch, WorkResourcePlan,
};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{
    RunnerDescriptorBuilder, RuntimeClient, RuntimeResult, TaskAwaitRunnerAdapter,
};
use serde_json::json;

use crate::measurement::{Sample, allocation_delta, allocation_snapshot};

pub fn pipeline_sample(event_count: usize, adapter_count: usize) -> Sample {
    let events = (0..event_count)
        .map(|index| benchmark_event(index, adapter_count, index % 2 == 0))
        .collect::<Vec<_>>();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut command = BotCommandNodeRunner::new(1);
    let mut echo = bot_echo::echo_runner(1);
    let mut command_hits = 0_usize;
    let mut message_outputs = 0_usize;
    let mut adapters = BTreeMap::<String, u64>::new();
    for chunk in events.chunks(64) {
        for event in chunk {
            *adapters.entry(event.bot.account_id.clone()).or_default() += 1;
        }
        let tasks = chunk
            .iter()
            .map(|event| {
                Task::new(
                    format!("command:{}", event.event_id),
                    BOT_COMMAND_PARSE_PROTOCOL_ID,
                    TaskPayload::from_local(command_invocation(event.clone())),
                )
            })
            .collect::<Vec<_>>();
        let completion = command
            .run_batch(
                context("command", tasks.len()),
                batch(BOT_COMMAND_RUNNER_ID, &tasks),
            )
            .unwrap();
        let echo_tasks = completion
            .results
            .into_iter()
            .map(|entry| {
                entry.result.unwrap_or_else(|| {
                    panic!("command node failed during benchmark: {:?}", entry.error)
                })
            })
            .filter_map(|result| result.output)
            .map(|value| serde_json::from_value::<BotNodeResult>(value).unwrap())
            .filter_map(|result| {
                let output = result.outputs.into_iter().next().unwrap();
                (output.port_id == "matched").then_some(output.event)
            })
            .enumerate()
            .map(|(ordinal, event)| {
                command_hits += 1;
                Task::new(
                    format!("echo:{ordinal}:{}", event.event_id),
                    bot_echo::ECHO_PROTOCOL_ID,
                    TaskPayload::from_local(BotNodeInvocation {
                        flow_id: "benchmark.chain".into(),
                        graph_revision: 1,
                        execution_id: format!("benchmark:{}", event.event_id),
                        node_id: "echo".into(),
                        input_port_id: "command".into(),
                        wiring: BotNodeWiring::default(),
                        config: json!({}),
                        input: event,
                    }),
                )
            })
            .collect::<Vec<_>>();
        if !echo_tasks.is_empty() {
            let completion = echo
                .run_batch(
                    context("echo", echo_tasks.len()),
                    batch(bot_echo::ECHO_RUNNER_ID, &echo_tasks),
                )
                .unwrap();
            message_outputs += completion
                .results
                .into_iter()
                .map(|entry| {
                    entry.result.unwrap_or_else(|| {
                        panic!("echo node failed during benchmark: {:?}", entry.error)
                    })
                })
                .filter_map(|result| result.output)
                .map(|value| serde_json::from_value::<BotNodeResult>(value).unwrap())
                .map(|result| result.outputs.len())
                .sum::<usize>();
        }
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let minimum = adapters.values().copied().min().unwrap_or(0);
    let maximum = adapters.values().copied().max().unwrap_or(1);
    let fairness = minimum as f64 / maximum.max(1) as f64;
    assert_eq!(command_hits, event_count.div_ceil(2));
    assert_eq!(message_outputs, command_hits);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 0,
        events: event_count as u64,
        queue_depth: event_count.min(64) as u64,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness,
        duplicate_executions: 0,
        retained_units: 0,
        output: json!({
            "events": event_count,
            "workload": "flow_three_node_chain",
            "commands": command_hits,
            "adapter_counts": adapters,
            "message_outputs": message_outputs
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn command_sample(hit: bool) -> Sample {
    let event = benchmark_event(7, 1, hit);
    let task = Task::new(
        "command-case",
        BOT_COMMAND_PARSE_PROTOCOL_ID,
        TaskPayload::from_local(command_invocation(event)),
    );
    let mut runner = BotCommandNodeRunner::new(1);
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let result = single_result(
        runner
            .run_batch(
                context("command-case", 1),
                batch(BOT_COMMAND_RUNNER_ID, std::slice::from_ref(&task)),
            )
            .unwrap(),
    )
    .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    let node_result: BotNodeResult = serde_json::from_value(result.output.unwrap()).unwrap();
    assert_eq!(node_result.outputs[0].port_id == "matched", hit);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 0,
        events: 1,
        queue_depth: 1,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 0,
        output: json!({
            "hit": hit,
            "output_port": node_result.outputs[0].port_id
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn handler_filter_sample(event_count: usize, handler_count: usize) -> Sample {
    let registry = benchmark_fanout_registry(handler_count);
    let mut runner = BotFlowIngressRunner::new(registry);
    let tasks = (0..event_count)
        .map(|index| {
            let event = benchmark_event(index, 1, true);
            Task::new(
                format!("fanout:{index}"),
                BOT_FLOW_INGRESS_PROTOCOL_ID,
                TaskPayload::from_local(flow_envelope(event)),
            )
        })
        .collect::<Vec<_>>();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut matches = 0_usize;
    for chunk in tasks.chunks(64) {
        let completion = runner
            .run_batch(
                context("flow-fanout", chunk.len()),
                batch(BOT_FLOW_INGRESS_RUNNER_ID, chunk),
            )
            .unwrap();
        matches += completion
            .results
            .into_iter()
            .filter_map(|entry| entry.result)
            .map(|result| result.tasks.len())
            .sum::<usize>();
    }
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(matches, event_count * handler_count);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    compact_sample(
        elapsed_ns,
        event_count as u64,
        json!({"workload": "flow_explicit_fanout", "branches": matches}),
        allocations,
        allocated_bytes,
    )
}

fn command_invocation(event: BotEvent) -> BotNodeInvocation {
    let event_id = event.event_id.clone();
    BotNodeInvocation {
        flow_id: "benchmark.chain".into(),
        graph_revision: 1,
        execution_id: format!("benchmark:{event_id}"),
        node_id: "command".into(),
        input_port_id: "event".into(),
        wiring: BotNodeWiring::default(),
        config: json!({
            "prefixes": ["/"],
            "path": ["echo"],
            "aliases": [],
            "arguments": [{
                "name": "text",
                "kind": "string",
                "optional": false,
                "variadic": true
            }],
            "case_sensitive": false
        }),
        input: flow_envelope(event),
    }
}

fn flow_envelope(event: BotEvent) -> BotFlowEventEnvelope {
    BotFlowEventEnvelope {
        event_id: event.event_id.clone(),
        protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
        payload: BotFlowPayload {
            event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
            value: serde_json::to_value(&event).unwrap(),
        },
        context: BotFlowContext {
            bot: Some(event.bot.clone()),
            target: Some(event.target.clone()),
            actor: event.actor.clone(),
            ext: event.ext.clone(),
        },
        trace_id: None,
        correlation_id: None,
    }
}

fn benchmark_fanout_registry(branch_count: usize) -> Arc<BotFlowRegistry> {
    let manifests = vec![
        qqbot_adapter_manifest(1, false),
        flow_router_manifest(),
        bot_command_manifest(1),
    ];
    let catalog = BotNodeCatalog::from_manifests(&manifests).unwrap();
    let mut nodes = vec![BotFlowNode {
        node_id: "source".into(),
        node_type_id: mutsuki_plugin_bot_adapter_qqbot::tasks::QQ_NODE_MESSAGE_CREATED.into(),
        node_type_version: 1,
        config: json!({}),
        source: Some(BotFlowSourceSelector {
            protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
            event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
        }),
        position: BotFlowNodePosition::default(),
    }];
    let mut edges = Vec::with_capacity(branch_count);
    for index in 0..branch_count {
        let node_id = format!("command-{index}");
        nodes.push(BotFlowNode {
            node_id: node_id.clone(),
            node_type_id: BOT_COMMAND_MATCH_NODE_TYPE_ID.into(),
            node_type_version: 1,
            config: json!({
                "prefixes": ["/"], "path": ["echo"], "aliases": [], "arguments": []
            }),
            source: None,
            position: BotFlowNodePosition::default(),
        });
        edges.push(BotFlowEdge {
            edge_id: format!("branch-{index}"),
            from_node_id: "source".into(),
            from_port_id: "event".into(),
            to_node_id: node_id,
            to_port_id: "event".into(),
            kind: BotFlowEdgeKind::Event,
        });
    }
    Arc::new(
        BotFlowRegistry::with_snapshot(
            catalog,
            BotFlowSnapshot {
                revision: 1,
                flow: BotFlowDocument {
                    flow_id: "benchmark.fanout".into(),
                    name: "explicit fan-out".into(),
                    nodes,
                    edges,
                },
            },
        )
        .unwrap(),
    )
}

pub fn conversation_sample(event_count: usize) -> Sample {
    let repository = Arc::new(BenchmarkConversationRepository::default());
    let service = ConversationService::new(repository, benchmark_policy());
    let event = benchmark_event(7, 1, true);
    let conversation = mutsuki_bot_conversation::qq_conversation_from_event(&event).unwrap();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let session_id = futures::executor::block_on(async {
        let mut session_id = String::new();
        for _ in 0..event_count {
            let resolved = service.resolve_execution(conversation.clone()).unwrap();
            session_id = service
                .get_or_create_session_binding(&resolved, Some("user-7"))
                .await
                .unwrap()
                .session_id;
        }
        session_id
    });
    let elapsed_ns = started.elapsed().as_nanos();
    assert!(!session_id.is_empty());
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    compact_sample(
        elapsed_ns,
        event_count as u64,
        json!({"session_id": session_id, "stable": true}),
        allocations,
        allocated_bytes,
    )
}

pub fn delivery_idempotency_sample(delivery_count: usize) -> Sample {
    let repository = Arc::new(BenchmarkDeliveryRepository::default());
    let gateway = Arc::new(BenchmarkDeliveryGateway::default());
    let service = ActiveDeliveryService::new(
        repository,
        gateway.clone(),
        Arc::new(BenchmarkDeliveryPolicy),
    );
    let conversation =
        mutsuki_bot_conversation::qq_conversation_from_event(&benchmark_event(7, 1, true)).unwrap();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    futures::executor::block_on(async {
        for index in 0..delivery_count {
            let request = BotActiveDeliveryRequest {
                delivery_id: format!("delivery-{index}"),
                idempotency_key: format!("delivery-key-{index}"),
                conversation: conversation.clone(),
                content: BotDeliveryContent {
                    segments: vec![MessageSegment::text("benchmark")],
                    summary: None,
                    reply_to: None,
                },
                policy: DeliveryPolicy {
                    max_attempts: 3,
                    initial_backoff_ms: 10,
                    max_backoff_ms: 1_000,
                    not_before_unix_ms: None,
                    expires_at_unix_ms: None,
                },
                dry_run: false,
                source_execution_id: None,
            };
            service.submit(&request, index as u64).await.unwrap();
            service.submit(&request, index as u64).await.unwrap();
        }
    });
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(*gateway.calls.lock().unwrap(), delivery_count as u64);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    compact_sample(
        elapsed_ns,
        delivery_count as u64,
        json!({"deliveries": delivery_count, "duplicate_sends": 0}),
        allocations,
        allocated_bytes,
    )
}

pub fn interaction_transition_sample(session_count: usize) -> Sample {
    let repository = Arc::new(BenchmarkInteractionRepository::default());
    let service = InteractionService::new(repository, Arc::new(BenchmarkInteractionMatcher));
    let event = benchmark_event(7, 1, true);
    let conversation = mutsuki_bot_conversation::qq_conversation_from_event(&event).unwrap();
    let actor_id = event.actor.as_ref().unwrap().user_id.clone();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    futures::executor::block_on(async {
        for index in 0..session_count {
            service
                .create(BotInteractionSession {
                    session_id: format!("interaction-{index}"),
                    conversation: conversation.clone(),
                    scope: InteractionScope::ActorInConversation,
                    actor_id: Some(actor_id.clone()),
                    state_ref_id: format!("state-{index}"),
                    wait: InteractionWaitSpec {
                        event_kinds: vec![BotEventKind::MessageCreated],
                        command: None,
                        predicate_service_id: None,
                        timeout_at_unix_ms: u64::MAX,
                        retry_prompt: None,
                    },
                    status: InteractionStatus::Waiting,
                    generation: 1,
                    version: 1,
                    exclusive: false,
                    retries_remaining: 1,
                })
                .unwrap();
            let mut next = event.clone();
            next.event_id = format!("interaction-event-{index}");
            assert!(
                service
                    .match_event(&next, index as u64)
                    .unwrap()
                    .unwrap()
                    .accepted
            );
        }
    });
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    compact_sample(
        elapsed_ns,
        session_count as u64,
        json!({"sessions": session_count, "matched": session_count}),
        allocations,
        allocated_bytes,
    )
}

pub fn sandbox_persist_sample(message_count: usize) -> Sample {
    let root = tempfile::tempdir().unwrap();
    let repository = Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
    let service = SandboxService::with_history("qq-main", repository).unwrap();
    let events = (0..message_count)
        .map(|index| benchmark_event(index, 1, false))
        .collect::<Vec<_>>();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    for event in events {
        service.observe_event(event);
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    compact_sample(
        elapsed_ns,
        message_count as u64,
        json!({"messages": message_count, "persist": "incremental"}),
        allocations,
        allocated_bytes,
    )
}

pub fn delivery_claim_due_sample(receipt_count: usize) -> Sample {
    let root = tempfile::tempdir().unwrap();
    let repository = Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
    let conversation =
        mutsuki_bot_conversation::qq_conversation_from_event(&benchmark_event(7, 1, true)).unwrap();
    futures::executor::block_on(async {
        for index in 0..receipt_count {
            let request = BotActiveDeliveryRequest {
                delivery_id: format!("claim-{index}"),
                idempotency_key: format!("claim-key-{index}"),
                conversation: conversation.clone(),
                content: BotDeliveryContent {
                    segments: vec![MessageSegment::text("benchmark")],
                    summary: None,
                    reply_to: None,
                },
                policy: DeliveryPolicy {
                    max_attempts: 3,
                    initial_backoff_ms: 10,
                    max_backoff_ms: 1_000,
                    not_before_unix_ms: None,
                    expires_at_unix_ms: None,
                },
                dry_run: false,
                source_execution_id: None,
            };
            repository.reserve(&request).await.unwrap();
        }
    });
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let claimed = futures::executor::block_on(async {
        let mut claimed = 0_usize;
        loop {
            let batch = repository.claim_due_delivery_ids(1_000_000).await.unwrap();
            if batch.is_empty() {
                break;
            }
            claimed += batch.len();
        }
        claimed
    });
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(claimed, receipt_count);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    compact_sample(
        elapsed_ns,
        receipt_count as u64,
        json!({"receipts": receipt_count, "claimed": claimed}),
        allocations,
        allocated_bytes,
    )
}

pub fn gateway_ingress_chain_sample(event_count: usize) -> Sample {
    let registry = benchmark_chain_registry();
    let mut mapper = QqGatewayMapRunner::new(1, "benchmark-adapter-00");
    let mut ingress = BotFlowIngressRunner::new(registry);
    let mut command = BotCommandNodeRunner::new(1);
    let mut echo = bot_echo::echo_runner(1);
    let frames = (0..event_count)
        .map(|index| {
            let frame: GatewayFrame =
                serde_json::from_value(benchmark_gateway_frame(index, 1, true)).unwrap();
            Task::new(
                format!("gateway:{index}"),
                QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
                TaskPayload::from_local(frame),
            )
        })
        .collect::<Vec<_>>();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut submitted = 0_usize;
    let mut command_hits = 0_usize;
    let mut echo_outputs = 0_usize;
    for chunk in frames.chunks(64) {
        let mapped_tasks = mapper
            .run_batch(
                context("gateway-map", chunk.len()),
                batch(
                    mutsuki_plugin_bot_adapter_qqbot::tasks::QQBOT_GATEWAY_RUNNER_ID,
                    chunk,
                ),
            )
            .unwrap();
        let ingress_tasks = mapped_tasks
            .results
            .into_iter()
            .flat_map(|entry| entry.result.expect("gateway map failed").tasks)
            .collect::<Vec<_>>();
        submitted += ingress_tasks.len();
        if ingress_tasks.is_empty() {
            continue;
        }
        let ingress_completion = ingress
            .run_batch(
                context("flow-ingress", ingress_tasks.len()),
                batch(BOT_FLOW_INGRESS_RUNNER_ID, &ingress_tasks),
            )
            .unwrap();
        let chain_children = ingress_completion
            .results
            .iter()
            .map(|entry| entry.result.as_ref().expect("ingress failed").tasks.len())
            .sum::<usize>();
        assert_eq!(chain_children, ingress_tasks.len());
        let events = ingress_tasks
            .iter()
            .map(|task| {
                let envelope = task
                    .payload
                    .decode_shared::<BotFlowEventEnvelope>()
                    .unwrap();
                serde_json::from_value::<BotEvent>(envelope.payload.value.clone()).unwrap()
            })
            .collect::<Vec<_>>();
        let command_tasks = events
            .into_iter()
            .map(|event| {
                Task::new(
                    format!("command:{}", event.event_id),
                    BOT_COMMAND_PARSE_PROTOCOL_ID,
                    TaskPayload::from_local(command_invocation(event)),
                )
            })
            .collect::<Vec<_>>();
        let completion = command
            .run_batch(
                context("command", command_tasks.len()),
                batch(BOT_COMMAND_RUNNER_ID, &command_tasks),
            )
            .unwrap();
        let echo_tasks = completion
            .results
            .into_iter()
            .map(|entry| {
                entry
                    .result
                    .unwrap_or_else(|| panic!("command node failed: {:?}", entry.error))
            })
            .filter_map(|result| result.output)
            .map(|value| serde_json::from_value::<BotNodeResult>(value).unwrap())
            .filter_map(|result| {
                let output = result.outputs.into_iter().next().unwrap();
                (output.port_id == "matched").then_some(output.event)
            })
            .enumerate()
            .map(|(ordinal, event)| {
                command_hits += 1;
                Task::new(
                    format!("echo:{ordinal}:{}", event.event_id),
                    bot_echo::ECHO_PROTOCOL_ID,
                    TaskPayload::from_local(BotNodeInvocation {
                        flow_id: "benchmark.gateway-ingress-chain".into(),
                        graph_revision: 1,
                        execution_id: format!("benchmark:{}", event.event_id),
                        node_id: "echo".into(),
                        input_port_id: "command".into(),
                        wiring: BotNodeWiring::default(),
                        config: json!({}),
                        input: event,
                    }),
                )
            })
            .collect::<Vec<_>>();
        if !echo_tasks.is_empty() {
            let completion = echo
                .run_batch(
                    context("echo", echo_tasks.len()),
                    batch(bot_echo::ECHO_RUNNER_ID, &echo_tasks),
                )
                .unwrap();
            echo_outputs += completion
                .results
                .into_iter()
                .map(|entry| {
                    entry
                        .result
                        .unwrap_or_else(|| panic!("echo node failed: {:?}", entry.error))
                })
                .filter_map(|result| result.output)
                .map(|value| serde_json::from_value::<BotNodeResult>(value).unwrap())
                .map(|result| result.outputs.len())
                .sum::<usize>();
        }
    }
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(submitted, event_count);
    assert_eq!(command_hits, event_count);
    assert_eq!(echo_outputs, event_count);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    compact_sample(
        elapsed_ns,
        event_count as u64,
        json!({
            "events": event_count,
            "nodes": 3,
            "ingress_tasks": submitted,
            "command_hits": command_hits,
            "echo_outputs": echo_outputs
        }),
        allocations,
        allocated_bytes,
    )
}

fn benchmark_chain_registry() -> Arc<BotFlowRegistry> {
    let manifests = vec![
        qqbot_adapter_manifest(1, false),
        flow_router_manifest(),
        bot_command_manifest(1),
        bot_echo::echo_manifest(1),
    ];
    let catalog = BotNodeCatalog::from_manifests(&manifests).unwrap();
    Arc::new(
        BotFlowRegistry::with_snapshot(
            catalog,
            BotFlowSnapshot {
                revision: 1,
                flow: BotFlowDocument {
                    flow_id: "benchmark.gateway-ingress-chain".into(),
                    name: "gateway ingress three-node chain".into(),
                    nodes: vec![
                        BotFlowNode {
                            node_id: "source".into(),
                            node_type_id:
                                mutsuki_plugin_bot_adapter_qqbot::tasks::QQ_NODE_MESSAGE_CREATED
                                    .into(),
                            node_type_version: 1,
                            config: json!({}),
                            source: Some(BotFlowSourceSelector {
                                protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                                event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
                            }),
                            position: BotFlowNodePosition::default(),
                        },
                        BotFlowNode {
                            node_id: "command".into(),
                            node_type_id: BOT_COMMAND_MATCH_NODE_TYPE_ID.into(),
                            node_type_version: 1,
                            config: json!({
                                "prefixes": ["/"], "path": ["echo"], "aliases": [], "arguments": []
                            }),
                            source: None,
                            position: BotFlowNodePosition::default(),
                        },
                        BotFlowNode {
                            node_id: "echo".into(),
                            node_type_id: "example.bot.echo".into(),
                            node_type_version: 1,
                            config: json!({}),
                            source: None,
                            position: BotFlowNodePosition::default(),
                        },
                    ],
                    edges: vec![
                        BotFlowEdge {
                            edge_id: "source-command".into(),
                            from_node_id: "source".into(),
                            from_port_id: "event".into(),
                            to_node_id: "command".into(),
                            to_port_id: "event".into(),
                            kind: BotFlowEdgeKind::Event,
                        },
                        BotFlowEdge {
                            edge_id: "command-echo".into(),
                            from_node_id: "command".into(),
                            from_port_id: "matched".into(),
                            to_node_id: "echo".into(),
                            to_port_id: "command".into(),
                            kind: BotFlowEdgeKind::Event,
                        },
                    ],
                },
            },
        )
        .unwrap(),
    )
}

fn compact_sample(
    elapsed_ns: u128,
    events: u64,
    output: serde_json::Value,
    allocations: u64,
    allocated_bytes: u64,
) -> Sample {
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 0,
        events,
        queue_depth: 1,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 1,
        output,
        allocations,
        allocated_bytes,
    }
}

#[derive(Default)]
struct BenchmarkConversationRepository {
    binding: Mutex<Option<AgentSessionBinding>>,
    events: Mutex<BTreeMap<(String, String), bool>>,
}

#[async_trait]
impl ConversationRepository for BenchmarkConversationRepository {
    async fn session_binding(
        &self,
        _: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        Ok(self.binding.lock().unwrap().clone())
    }

    async fn compare_and_set_session_binding(
        &self,
        _: &str,
        expected: Option<u64>,
        binding: AgentSessionBinding,
    ) -> Result<(), ConversationError> {
        let mut current = self.binding.lock().unwrap();
        if current.as_ref().map(|value| value.generation) != expected {
            return Err(ConversationError::GenerationConflict);
        }
        *current = Some(binding);
        Ok(())
    }

    async fn begin_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
        _: &str,
    ) -> Result<mutsuki_bot_conversation::AgentEventClaim, ConversationError> {
        use mutsuki_bot_conversation::AgentEventClaim;
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
struct BenchmarkDeliveryRepository {
    requests: Mutex<BTreeMap<String, BotActiveDeliveryRequest>>,
    keys: Mutex<BTreeMap<String, String>>,
    receipts: Mutex<BTreeMap<String, BotDeliveryReceipt>>,
}

#[async_trait]
impl DeliveryRepository for BenchmarkDeliveryRepository {
    async fn reserve(
        &self,
        request: &BotActiveDeliveryRequest,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError> {
        if let Some(delivery_id) = self.keys.lock().unwrap().get(&request.idempotency_key) {
            return Ok(self.receipts.lock().unwrap().get(delivery_id).cloned());
        }
        self.keys
            .lock()
            .unwrap()
            .insert(request.idempotency_key.clone(), request.delivery_id.clone());
        self.requests
            .lock()
            .unwrap()
            .insert(request.delivery_id.clone(), request.clone());
        self.receipts.lock().unwrap().insert(
            request.delivery_id.clone(),
            BotDeliveryReceipt {
                delivery_id: request.delivery_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                status: DeliveryStatus::Pending,
                attempt_count: 0,
                platform_message_ids: Vec::new(),
                part_receipts: Vec::new(),
                delivered_at_unix_ms: None,
                error_code: None,
                generation: 0,
                lease_expires_at_unix_ms: None,
            },
        );
        Ok(None)
    }

    async fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
        self.requests
            .lock()
            .unwrap()
            .get(delivery_id)
            .cloned()
            .ok_or(DeliveryError::NotFound)
    }

    async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        self.receipts
            .lock()
            .unwrap()
            .get(delivery_id)
            .cloned()
            .ok_or(DeliveryError::NotFound)
    }

    async fn attempts(&self, _delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
        Ok(Vec::new())
    }

    async fn save_outcome(
        &self,
        _attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
    ) -> Result<(), DeliveryError> {
        self.receipts
            .lock()
            .unwrap()
            .insert(receipt.delivery_id.clone(), receipt);
        Ok(())
    }

    async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
        self.receipts
            .lock()
            .unwrap()
            .insert(receipt.delivery_id.clone(), receipt);
        Ok(())
    }

    async fn claim_due_delivery_ids(
        &self,
        _now_unix_ms: u64,
    ) -> Result<Vec<String>, DeliveryError> {
        Ok(Vec::new())
    }

    async fn begin_send(
        &self,
        delivery_id: &str,
        attempt: BotDeliveryAttempt,
        now_unix_ms: u64,
        lease_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let _ = attempt;
        let mut receipts = self.receipts.lock().unwrap();
        let receipt =
            receipts
                .entry(delivery_id.to_owned())
                .or_insert_with(|| BotDeliveryReceipt {
                    delivery_id: delivery_id.to_owned(),
                    idempotency_key: delivery_id.to_owned(),
                    status: DeliveryStatus::Pending,
                    attempt_count: 0,
                    platform_message_ids: Vec::new(),
                    part_receipts: Vec::new(),
                    delivered_at_unix_ms: None,
                    error_code: None,
                    generation: 0,
                    lease_expires_at_unix_ms: None,
                });
        receipt.status = DeliveryStatus::Sending;
        receipt.generation = receipt.generation.saturating_add(1);
        receipt.lease_expires_at_unix_ms = Some(now_unix_ms.saturating_add(lease_ms));
        Ok(receipt.clone())
    }
}

#[derive(Default)]
struct BenchmarkDeliveryGateway {
    calls: Mutex<u64>,
}

impl DeliveryGateway for BenchmarkDeliveryGateway {
    fn send(
        &self,
        _target: &BotTarget,
        _content: &BotDeliveryContent,
    ) -> Result<DeliverySuccess, DeliveryFailure> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        Ok(DeliverySuccess {
            platform_message_ids: vec![format!("message-{calls}")],
            part_receipts: Vec::new(),
        })
    }
}

struct BenchmarkDeliveryPolicy;

impl DeliveryPolicyResolver for BenchmarkDeliveryPolicy {
    fn active_delivery_allowed(
        &self,
        _conversation: &QqConversationRef,
    ) -> Result<bool, DeliveryError> {
        Ok(true)
    }
}

#[derive(Default)]
struct BenchmarkInteractionRepository {
    sessions: Mutex<BTreeMap<String, BotInteractionSession>>,
}

impl InteractionRepository for BenchmarkInteractionRepository {
    fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.session_id.clone(), session);
        Ok(())
    }

    fn active_for_origin(
        &self,
        origin_key: &str,
    ) -> Result<Vec<BotInteractionSession>, InteractionError> {
        Ok(self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|session| {
                session.status == InteractionStatus::Waiting
                    && session.conversation.origin_key() == origin_key
            })
            .cloned()
            .collect())
    }

    fn compare_and_set(
        &self,
        expected_version: u64,
        session: BotInteractionSession,
    ) -> Result<(), InteractionError> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions
            .get(&session.session_id)
            .map(|current| current.version)
            != Some(expected_version)
        {
            return Err(InteractionError::GenerationConflict);
        }
        sessions.insert(session.session_id.clone(), session);
        Ok(())
    }

    fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError> {
        Ok(self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|session| session.status == InteractionStatus::Waiting)
            .cloned()
            .collect())
    }
}

struct BenchmarkInteractionMatcher;

impl InteractionConditionMatcher for BenchmarkInteractionMatcher {
    fn command_matches(&self, _command: &str, _event: &BotEvent) -> Result<bool, InteractionError> {
        Ok(true)
    }

    fn predicate_matches(
        &self,
        _service_id: &str,
        _event: &BotEvent,
    ) -> Result<bool, InteractionError> {
        Ok(true)
    }
}

fn benchmark_policy() -> ConversationPolicy {
    ConversationPolicy {
        revision: 1,
        session_scope: AgentSessionScope::SharedConversation,
        business_profile_binding_id: None,
        agent_runtime_profile_id: Some("benchmark".into()),
        stt_enabled: false,
        tts_enabled: false,
        speech_reply_policy: Default::default(),
        stt_selector_id: None,
        tts_selector_id: None,
        active_delivery_enabled: true,
    }
}

pub fn link_parse_sample() -> Sample {
    let payload = benchmark_card_payload();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let card_urls = expand_card_payload(&payload).unwrap();
    let text_urls = extract_urls(
        "fixed https://b23.tv/fixed repeated https://b23.tv/fixed and https://example.com/x",
    );
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(card_urls.len(), 4);
    assert_eq!(text_urls.len(), 2);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 0,
        events: 1,
        queue_depth: 0,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 0,
        output: json!({
            "card_urls": card_urls.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "text_urls": text_urls.iter().map(ToString::to_string).collect::<Vec<_>>()
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn duplicate_sample() -> Sample {
    let raw = benchmark_gateway_frame(1, 1, true);
    let mut pump = QqGatewayPump::with_account("benchmark", 32);
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let first = pump.handle_raw_frame(raw.clone(), 1).unwrap();
    let duplicate = pump.handle_raw_frame(raw, 1).unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    assert!(first.is_some());
    assert!(duplicate.is_none());
    let dispatch_actions = std::iter::from_fn(|| pump.pop_action())
        .filter(|action| matches!(action, GatewayAction::DispatchTask(_)))
        .count();
    assert_eq!(dispatch_actions, 1);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 0,
        events: 2,
        queue_depth: 1,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 1,
        output: json!({
            "first_task": first.unwrap().task_id,
            "dispatch_actions": dispatch_actions,
            "duplicate_executions": 0
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn long_run_sample(event_count: usize) -> Sample {
    const WINDOW: usize = 2_048;
    let mut pump = QqGatewayPump::with_account("long-run", WINDOW);
    // Fixture construction is not Bot orchestration. Keep it outside the measured region, as the
    // event-burst cases do, so the budget measures the Gateway pump rather than serde_json::json!.
    let frames = (0..event_count)
        .map(|index| benchmark_gateway_frame(index, 4, false))
        .collect::<Vec<_>>();
    let old = benchmark_gateway_frame(0, 4, false);
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut accepted = 0_u64;
    for raw in frames {
        if pump.handle_raw_frame(raw, 1).unwrap().is_some() {
            accepted += 1;
        }
        let _ = pump.pop_action();
    }
    assert!(pump.handle_raw_frame(old.clone(), 1).unwrap().is_some());
    let _ = pump.pop_action();
    assert!(pump.handle_raw_frame(old, 1).unwrap().is_none());
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(accepted, event_count as u64);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 0,
        events: event_count as u64,
        queue_depth: 1,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: WINDOW as u64,
        output: json!({
            "events": event_count,
            "accepted": accepted,
            "dedup_window": WINDOW,
            "old_event_evicted_then_reserved": true,
            "last_sequence": pump.last_sequence()
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn rate_limit_sample() -> Sample {
    let requests = Arc::new(Mutex::new(0_u64));
    let responses = VecDeque::from([
        QqHttpResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: json!({"access_token": "BENCHMARK_TOKEN", "expires_in": 7200}),
        },
        QqHttpResponse {
            status: 429,
            headers: BTreeMap::from([("Retry-After".into(), "0.001".into())]),
            body: json!({"code": 429, "message": "benchmark rate limit"}),
        },
        QqHttpResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: json!({"id": "BENCHMARK_REPLY"}),
        },
    ]);
    let mut config = QqBotConfig::new("benchmark", "BENCHMARK_APP");
    config.max_retry_attempts = 3;
    config.retry_base_delay_ms = 0;
    config.retry_max_delay_ms = 10;
    let client = ScriptedHttpClient {
        responses,
        requests: requests.clone(),
    };
    let mut transport = QqOpenApiTransport::new(
        config,
        Box::new(client),
        Arc::new(StaticQqCredentials::new("BENCHMARK_SECRET")),
    );
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let output = transport
        .execute_json(
            HttpMethod::Post,
            "/v2/groups/G/messages".into(),
            json!({"fixed": true}),
        )
        .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(*requests.lock().unwrap(), 3);
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 1_000_000,
        events: 1,
        queue_depth: 1,
        dropped: 0,
        deferred: 1,
        retried: 1,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 0,
        output: json!({"response": output, "requests": 3, "retry_after_ms": 1}),
        allocations,
        allocated_bytes,
    }
}

pub fn wait_resume_sample() -> Sample {
    let client = Arc::new(OutcomeClient::default());
    let descriptor = RunnerDescriptorBuilder::new("benchmark.wait.runner", "benchmark.bot")
        .accepted_protocol("mutsuki.bot.benchmark/wait@1")
        .requires_protocol("mutsuki.bot.benchmark/result@1")
        .build();
    let mut runner = TaskAwaitRunnerAdapter::new(
        descriptor,
        client.clone(),
        Box::new(|ctx, task| {
            Box::pin(async move {
                let outcome = ctx
                    .call_raw("mutsuki.bot.benchmark/result@1", json!({"fixed": true}))
                    .await?
                    .into_outcome();
                let mut result = RunnerResult::completed(task.task_id);
                result.output = match outcome {
                    TaskOutcome::Completed { output, .. } => output,
                    other => Some(json!({"unexpected": format!("{other:?}")})),
                };
                Ok(result)
            })
        }),
    );
    let task = Task::new(
        "benchmark-wait",
        "mutsuki.bot.benchmark/wait@1",
        json!({"seed": BENCHMARK_FIXED_SEED}),
    );
    let batch = batch("benchmark.wait.runner", std::slice::from_ref(&task));
    let ctx = context("wait", 1);
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let first = single_result(runner.run_batch(ctx.clone(), batch.clone()).unwrap()).unwrap();
    assert_eq!(first.status, RunnerStatus::Waiting);
    let child = first.tasks.into_iter().next().unwrap();
    let empty_poll = single_result(runner.run_batch(ctx.clone(), batch.clone()).unwrap()).unwrap();
    assert_eq!(empty_poll.status, RunnerStatus::Waiting);
    assert!(empty_poll.tasks.is_empty());
    client.complete(TaskOutcome::Completed {
        task_id: child.task_id,
        output: Some(json!({"fixed": true, "seed": BENCHMARK_FIXED_SEED})),
        output_ref: None,
    });
    let completed = single_result(runner.run_batch(ctx, batch).unwrap()).unwrap();
    assert_eq!(completed.status, RunnerStatus::Completed);
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 0,
        events: 1,
        queue_depth: 1,
        dropped: 0,
        deferred: 1,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 0,
        output: completed.output.unwrap(),
        allocations,
        allocated_bytes,
    }
}

struct ScriptedHttpClient {
    responses: VecDeque<QqHttpResponse>,
    requests: Arc<Mutex<u64>>,
}

impl QqHttpClient for ScriptedHttpClient {
    fn send(&mut self, _request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError> {
        *self.requests.lock().unwrap() += 1;
        self.responses
            .pop_front()
            .ok_or_else(|| QqOpenApiError::InvalidResponse("benchmark response exhausted".into()))
    }
}

#[derive(Default)]
struct OutcomeClient {
    outcomes: Mutex<BTreeMap<TaskId, TaskOutcome>>,
}

impl OutcomeClient {
    fn complete(&self, outcome: TaskOutcome) {
        let task_id = match &outcome {
            TaskOutcome::Completed { task_id, .. }
            | TaskOutcome::Failed { task_id, .. }
            | TaskOutcome::Cancelled { task_id, .. }
            | TaskOutcome::Expired { task_id, .. }
            | TaskOutcome::DeadLetter { task_id, .. } => task_id.clone(),
        };
        self.outcomes.lock().unwrap().insert(task_id, outcome);
    }
}

impl RuntimeClient for OutcomeClient {
    fn submit_batch(&self, _batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        Ok(Vec::new())
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(self.outcomes.lock().unwrap().get(&handle.task_id).cloned())
    }
}

fn single_result(completion: CompletionBatch) -> Result<RunnerResult, RuntimeError> {
    let entry = completion.results.into_iter().next().unwrap();
    match (entry.result, entry.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(error),
        _ => Err(RuntimeError::new(
            "bot.benchmark.invalid_completion",
            "bot.benchmark",
            entry.task_id,
        )),
    }
}

fn context(id: &str, entries: usize) -> RunnerContext {
    RunnerContext::new(1, 1, "bot-benchmark", None::<&str>, id)
        .with_batch(format!("batch:{id}"), entries)
}

fn batch(runner_id: &str, tasks: &[Task]) -> WorkBatch {
    WorkBatch {
        batch_id: format!("batch:{}", tasks[0].task_id).into(),
        tick_id: "tick:bot-benchmark".into(),
        batch_key: runner_id.into(),
        entries: tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: task.task_id.as_str().into(),
                task_id: task.task_id.clone(),
                trace_id: task.trace_id.clone(),
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(index),
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::PreserveSubmitOrder,
            })
            .collect(),
        payload: BatchPayload::from_task_refs(tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: Vec::new(),
    }
}
