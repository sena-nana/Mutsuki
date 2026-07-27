use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use mutsuki_agent_client::{AgentClient, AgentLinkClient, AgentLinkServer, InProcessAgentService};
use mutsuki_agent_contracts::*;
use mutsuki_agent_distributed_integration::{AgentCapabilityResolver, distributed_request};
use mutsuki_agent_plugin_lsp::{LspProcess, LspProcessFactory, SharedLspService};
use mutsuki_agent_testkit::{
    BENCHMARK_FIXED_SEED, BENCHMARK_MODEL_ID, BENCHMARK_TOOL_NAME, BENCHMARK_TOOL_PROTOCOL,
    BenchmarkModelProvider, SimulatedLatency, benchmark_tool_descriptor, execute_benchmark_tool,
};
use mutsuki_distributed_contracts::{CapabilityBits, NodeId};
use mutsuki_link_core::{EndpointId, MemoryTransportConfig, memory_transport_pair};
use mutsuki_plugin_agent_context::ContextBuilder;
use mutsuki_plugin_agent_loop::AgentLoop;
use mutsuki_plugin_agent_memory_router::MemoryRouter;
use mutsuki_plugin_agent_model_gateway::ModelGateway;
use mutsuki_plugin_agent_session::SessionStore;
use mutsuki_plugin_agent_tool_router::ToolRegistry;
use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CompletionBatch, DispatchLane, OrderingRequirement, ResourceAccess,
    ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic, RunnerContext, RunnerResult,
    RunnerStatus, RuntimeError, Task, TaskBatch, TaskHandle, TaskOutcome, WorkBatch,
    WorkResourcePlan,
};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{RuntimeClient, RuntimeResult};
use serde_json::{Value, json};

use crate::measurement::{Sample, allocation_delta, allocation_snapshot, canonical_hash};

#[derive(Default)]
struct OutcomeClient {
    outcomes: Mutex<BTreeMap<String, TaskOutcome>>,
}

impl OutcomeClient {
    fn complete(&self, outcome: TaskOutcome) {
        let task_id = match &outcome {
            TaskOutcome::Completed { task_id, .. }
            | TaskOutcome::Failed { task_id, .. }
            | TaskOutcome::Cancelled { task_id, .. }
            | TaskOutcome::Expired { task_id, .. }
            | TaskOutcome::DeadLetter { task_id, .. } => task_id,
        };
        self.outcomes
            .lock()
            .expect("benchmark outcome mutex poisoned")
            .insert(task_id.clone(), outcome);
    }
}

impl RuntimeClient for OutcomeClient {
    fn submit_batch(&self, _batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        Ok(Vec::new())
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(self
            .outcomes
            .lock()
            .expect("benchmark outcome mutex poisoned")
            .get(&handle.task_id)
            .cloned())
    }
}

#[derive(Default)]
struct RouteCounts {
    tasks: u64,
    continuations: u64,
    tool_routes: u64,
    max_tool_inflight: u64,
}

struct Harness {
    client: Arc<OutcomeClient>,
    gateway: ModelGateway,
    tools: ToolRegistry,
    sessions: SessionStore,
    context: ContextBuilder,
    latency: SimulatedLatency,
}

impl Harness {
    fn new(latency: SimulatedLatency) -> Self {
        let gateway = ModelGateway::with_default_provider(BENCHMARK_MODEL_ID);
        gateway.register(Arc::new(BenchmarkModelProvider::new(latency)));
        let tools = ToolRegistry::default();
        tools
            .register(benchmark_tool_descriptor())
            .expect("benchmark tool descriptor is valid");
        Self {
            client: Arc::new(OutcomeClient::default()),
            gateway,
            tools,
            sessions: SessionStore::default(),
            context: ContextBuilder::default(),
            latency,
        }
    }

    fn drive_agent(
        &self,
        task_id: &str,
        request: AgentRunRequest,
        delayed_resume: bool,
        counts: &mut RouteCounts,
    ) -> Result<Value, RuntimeError> {
        let mut runner = mutsuki_plugin_agent_loop::runner(
            self.client.clone(),
            AgentLoop::default().with_default_model(BENCHMARK_MODEL_ID),
        );
        let task = Task::new(
            task_id,
            AGENT_RUN_PROTOCOL,
            serde_json::to_value(request).unwrap(),
        );
        let batch = batch("mutsuki.agent.loop.runner", std::slice::from_ref(&task));
        let mut ctx = context(task_id);
        let mut held = None;
        let mut delay_pending = delayed_resume;
        for _ in 0..128 {
            let result = single_result(runner.run_batch(ctx.clone(), batch.clone()).unwrap())?;
            match result.status {
                RunnerStatus::Completed => {
                    return result.output.ok_or_else(|| {
                        RuntimeError::new(
                            "agent.benchmark.missing_output",
                            "agent.benchmark",
                            task_id,
                        )
                    });
                }
                RunnerStatus::Waiting | RunnerStatus::Continue => {
                    counts.continuations += 1;
                    if result.tasks.is_empty() {
                        if let Some(child) = held.take() {
                            self.dispatch(child, counts);
                        }
                    } else {
                        let tool_batch = result
                            .tasks
                            .iter()
                            .filter(|task| task.protocol_id == AGENT_TOOL_EXECUTE_PROTOCOL)
                            .count() as u64;
                        counts.max_tool_inflight = counts.max_tool_inflight.max(tool_batch);
                        for child in result.tasks {
                            counts.tasks += 1;
                            if delay_pending {
                                held = Some(child);
                                delay_pending = false;
                            } else {
                                self.dispatch(child, counts);
                            }
                        }
                    }
                }
                other => {
                    return Err(RuntimeError::new(
                        "agent.benchmark.unexpected_status",
                        "agent.benchmark",
                        format!("{task_id}:{other:?}"),
                    ));
                }
            }
            ctx.current_step = ctx.current_step.saturating_add(1);
        }
        Err(RuntimeError::new(
            "agent.benchmark.poll_limit",
            "agent.benchmark",
            task_id,
        ))
    }

    fn dispatch(&self, task: Task, counts: &mut RouteCounts) {
        let outcome = match task.protocol_id.as_str() {
            AGENT_MODEL_GENERATE_PROTOCOL => self.immediate_model(task),
            AGENT_TOOL_EXECUTE_PROTOCOL => self.route_tool(task, counts),
            AGENT_TOOL_LIST_PROTOCOL => self.immediate_tool(task),
            AGENT_CONTEXT_BUILD_PROTOCOL => self.immediate_context(task),
            AGENT_SESSION_GET_PROTOCOL
            | AGENT_SESSION_APPEND_PROTOCOL
            | AGENT_SESSION_CREATE_PROTOCOL
            | AGENT_SESSION_SNAPSHOT_PROTOCOL => self.immediate_session(task),
            other => TaskOutcome::Failed {
                task_id: task.task_id,
                error: RuntimeError::new("agent.benchmark.wrong_route", "agent.benchmark", other),
            },
        };
        self.client.complete(outcome);
    }

    fn immediate_model(&self, task: Task) -> TaskOutcome {
        let task_id = task.task_id.clone();
        let request = match serde_json::from_value::<AgentModelGenerateRequest>(task.payload.into())
        {
            Ok(request) => request,
            Err(error) => {
                return TaskOutcome::Failed {
                    task_id,
                    error: RuntimeError::new(
                        "agent.benchmark.model_decode",
                        "agent.benchmark",
                        error.to_string(),
                    ),
                };
            }
        };
        match self.gateway.generate(request) {
            Ok(output) => TaskOutcome::Completed {
                task_id,
                output: Some(serde_json::to_value(output).unwrap()),
                output_ref: None,
            },
            Err(error) => TaskOutcome::Failed {
                task_id: task_id.clone(),
                error: mutsuki_agent_sdk::runtime_failure("agent.benchmark", &task_id, error)
                    .error()
                    .clone(),
            },
        }
    }

    fn immediate_session(&self, task: Task) -> TaskOutcome {
        let mut runner =
            mutsuki_plugin_agent_session::runner(self.client.clone(), self.sessions.clone());
        immediate_outcome(&mut runner, task, mutsuki_plugin_agent_session::RUNNER_ID)
    }

    fn immediate_context(&self, task: Task) -> TaskOutcome {
        let mut runner =
            mutsuki_plugin_agent_context::runner(self.client.clone(), self.context.clone());
        immediate_outcome(&mut runner, task, mutsuki_plugin_agent_context::RUNNER_ID)
    }

    fn immediate_tool(&self, task: Task) -> TaskOutcome {
        let mut runner =
            mutsuki_plugin_agent_tool_router::runner(self.client.clone(), self.tools.clone());
        immediate_outcome(
            &mut runner,
            task,
            mutsuki_plugin_agent_tool_router::RUNNER_ID,
        )
    }

    fn route_tool(&self, task: Task, counts: &mut RouteCounts) -> TaskOutcome {
        counts.tool_routes += 1;
        let task_id = task.task_id.clone();
        let mut runner =
            mutsuki_plugin_agent_tool_router::runner(self.client.clone(), self.tools.clone());
        let batch = batch(
            mutsuki_plugin_agent_tool_router::RUNNER_ID,
            std::slice::from_ref(&task),
        );
        let ctx = context(&task_id);
        let first = match single_result(runner.run_batch(ctx.clone(), batch.clone()).unwrap()) {
            Ok(result) => result,
            Err(error) => return TaskOutcome::Failed { task_id, error },
        };
        counts.continuations += 1;
        let Some(target) = first.tasks.into_iter().next() else {
            return TaskOutcome::Failed {
                task_id,
                error: RuntimeError::new(
                    "agent.benchmark.tool_target_missing",
                    "agent.benchmark",
                    "tool router did not emit target task",
                ),
            };
        };
        counts.tasks += 1;
        if target.protocol_id != BENCHMARK_TOOL_PROTOCOL {
            return TaskOutcome::Failed {
                task_id,
                error: RuntimeError::new(
                    "agent.benchmark.wrong_tool_route",
                    "agent.benchmark",
                    target.protocol_id,
                ),
            };
        }
        let executed = execute_benchmark_tool(
            AgentToolExecuteRequest {
                call_id: None,
                name: BENCHMARK_TOOL_NAME.into(),
                input: target.payload.into(),
                session_id: None,
                approval: None,
            },
            self.latency,
        );
        self.client.complete(TaskOutcome::Completed {
            task_id: target.task_id,
            output: executed.output,
            output_ref: None,
        });
        outcome_from_completion(runner.run_batch(ctx, batch).unwrap())
    }
}

pub fn agent_case_sample(scenario: &str, latency: SimulatedLatency) -> Sample {
    let harness = Harness::new(latency);
    let request = run_request(scenario, None, 9);
    let mut counts = RouteCounts::default();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let output = harness
        .drive_agent("agent-run", request, false, &mut counts)
        .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let (model_calls, tool_calls) = match scenario {
        "single-turn" => (1, 0),
        "tool-once" => (2, 1),
        "tool-chain-8" => (9, 8),
        _ => unreachable!("parallel tools use the batched path"),
    };
    let simulated_ns = u128::from(latency.micros()) * 1_000;
    Sample {
        elapsed_ns,
        simulated_wall_ns: simulated_ns * (model_calls + tool_calls),
        simulated_work_ns: simulated_ns * (model_calls + tool_calls),
        tasks: counts.tasks,
        continuations: counts.continuations,
        tool_routes: counts.tool_routes,
        max_tool_inflight: counts.max_tool_inflight,
        retained_bytes: 0,
        post_warmup_growth_bytes: 0,
        output,
        allocations,
        allocated_bytes,
    }
}

pub fn parallel_tools_sample(latency: SimulatedLatency) -> Sample {
    let harness = Harness::new(latency);
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let generated = harness
        .gateway
        .generate(model_request("parallel-tools-8", Vec::new()))
        .unwrap();
    assert_eq!(generated.tool_calls.len(), 8);
    let tool_tasks = generated
        .tool_calls
        .iter()
        .map(|call| {
            Task::new(
                format!("parallel-route:{}", call.call_id),
                AGENT_TOOL_EXECUTE_PROTOCOL,
                serde_json::to_value(AgentToolExecuteRequest {
                    call_id: Some(call.call_id.clone()),
                    name: call.name.clone(),
                    input: call.input.clone(),
                    session_id: None,
                    approval: None,
                })
                .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let tool_batch = batch(mutsuki_plugin_agent_tool_router::RUNNER_ID, &tool_tasks);
    let ctx = context("parallel-tools");
    let mut runner =
        mutsuki_plugin_agent_tool_router::runner(harness.client.clone(), harness.tools.clone());
    let first = runner.run_batch(ctx.clone(), tool_batch.clone()).unwrap();
    let targets = first
        .results
        .into_iter()
        .map(|entry| {
            entry
                .result
                .expect("parallel tool route succeeds")
                .tasks
                .into_iter()
                .next()
                .expect("parallel tool route emits target")
        })
        .collect::<Vec<_>>();
    assert!(
        targets
            .iter()
            .all(|task| task.protocol_id == BENCHMARK_TOOL_PROTOCOL)
    );
    let results = thread::scope(|scope| {
        targets
            .into_iter()
            .map(|target| {
                scope.spawn(move || {
                    let executed = execute_benchmark_tool(
                        AgentToolExecuteRequest {
                            call_id: None,
                            name: BENCHMARK_TOOL_NAME.into(),
                            input: target.payload.into(),
                            session_id: None,
                            approval: None,
                        },
                        latency,
                    );
                    (target.task_id, executed.output)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|join| join.join().unwrap())
            .collect::<Vec<_>>()
    });
    for (task_id, output) in results {
        harness.client.complete(TaskOutcome::Completed {
            task_id,
            output,
            output_ref: None,
        });
    }
    let routed = runner.run_batch(ctx, tool_batch).unwrap();
    let tool_outputs = routed
        .results
        .into_iter()
        .map(|entry| {
            entry
                .result
                .expect("parallel tool completes")
                .output
                .expect("parallel tool output")
        })
        .collect::<Vec<_>>();
    let messages = tool_outputs
        .iter()
        .enumerate()
        .map(|(index, output)| AgentMessage {
            role: AgentRole::Tool,
            content: output.to_string(),
            name: Some(BENCHMARK_TOOL_NAME.into()),
            metadata: Some(json!({"call_id": format!("benchmark-call-{index:02}")})),
        })
        .collect::<Vec<_>>();
    let final_result = harness
        .gateway
        .generate(model_request("parallel-tools-8", messages))
        .unwrap();
    assert!(final_result.tool_calls.is_empty());
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let simulated_ns = u128::from(latency.micros()) * 1_000;
    Sample {
        elapsed_ns,
        simulated_wall_ns: simulated_ns * 3,
        simulated_work_ns: simulated_ns * 10,
        tasks: 18,
        continuations: 8,
        tool_routes: 8,
        max_tool_inflight: 8,
        retained_bytes: 0,
        post_warmup_growth_bytes: 0,
        output: json!({"model": final_result, "tools": tool_outputs}),
        allocations,
        allocated_bytes,
    }
}

pub fn wait_resume_sample(latency: SimulatedLatency) -> Sample {
    let harness = Harness::new(latency);
    let mut counts = RouteCounts::default();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let output = harness
        .drive_agent(
            "agent-wait-resume",
            run_request("single-turn", None, 1),
            true,
            &mut counts,
        )
        .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let simulated_ns = u128::from(latency.micros()) * 1_000;
    Sample {
        elapsed_ns,
        simulated_wall_ns: simulated_ns,
        simulated_work_ns: simulated_ns,
        tasks: counts.tasks,
        continuations: counts.continuations,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes: 0,
        post_warmup_growth_bytes: 0,
        output,
        allocations,
        allocated_bytes,
    }
}

pub fn cancel_sample() -> Sample {
    let harness = Harness::new(SimulatedLatency::ZeroUs);
    let mut runner = mutsuki_plugin_agent_loop::runner(
        harness.client,
        AgentLoop::default().with_default_model(BENCHMARK_MODEL_ID),
    );
    let task = Task::new(
        "agent-cancel",
        AGENT_RUN_PROTOCOL,
        serde_json::to_value(run_request("single-turn", None, 1)).unwrap(),
    );
    let batch = batch("mutsuki.agent.loop.runner", std::slice::from_ref(&task));
    let mut ctx = context("agent-cancel");
    ctx.invocation_id = "agent-cancel-invocation".into();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let first = single_result(runner.run_batch(ctx.clone(), batch.clone()).unwrap()).unwrap();
    assert_eq!(first.status, RunnerStatus::Waiting);
    assert_eq!(first.tasks.len(), 1);
    runner.cancel(&ctx.invocation_id).unwrap();
    let restarted = single_result(runner.run_batch(ctx.clone(), batch).unwrap()).unwrap();
    assert_eq!(restarted.status, RunnerStatus::Waiting);
    assert_eq!(restarted.tasks.len(), 1);
    runner.cancel(&ctx.invocation_id).unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: 2,
        continuations: 2,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes: 0,
        post_warmup_growth_bytes: 0,
        output: json!({"status": "cancelled", "restart_verified": true}),
        allocations,
        allocated_bytes,
    }
}

pub fn failure_retry_sample(latency: SimulatedLatency) -> Sample {
    let harness = Harness::new(latency);
    let mut counts = RouteCounts::default();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let retryable = harness
        .drive_agent(
            "agent-failure-retryable",
            failure_request("retryable"),
            false,
            &mut counts,
        )
        .unwrap_err();
    assert_eq!(retryable.code, "agent.provider_unavailable");
    let recovered = harness
        .drive_agent(
            "agent-failure-recovered",
            run_request("single-turn", None, 1),
            false,
            &mut counts,
        )
        .unwrap();
    let fatal = harness
        .drive_agent(
            "agent-failure-fatal",
            failure_request("non-retryable"),
            false,
            &mut counts,
        )
        .unwrap_err();
    assert_eq!(fatal.code, "agent.invalid_input");
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let simulated_ns = u128::from(latency.micros()) * 1_000;
    Sample {
        elapsed_ns,
        simulated_wall_ns: simulated_ns * 3,
        simulated_work_ns: simulated_ns * 3,
        tasks: counts.tasks,
        continuations: counts.continuations,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes: 0,
        post_warmup_growth_bytes: 0,
        output: json!({
            "retryable": retryable.code,
            "recovered": recovered,
            "non_retryable": fatal.code
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn session_100_sample(latency: SimulatedLatency) -> Sample {
    let harness = Harness::new(latency);
    let session = harness
        .sessions
        .create(AgentSessionCreateRequest {
            profile_id: "benchmark.profile".into(),
            title: Some("benchmark-v1".into()),
        })
        .unwrap();
    let mut counts = RouteCounts::default();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = Value::Null;
    let mut warmup_retained_bytes = 0;
    for turn in 0..100 {
        last = harness
            .drive_agent(
                &format!("agent-session-turn-{turn:03}"),
                run_request("single-turn", Some(session.session_id.clone()), 1),
                false,
                &mut counts,
            )
            .unwrap();
        if turn == 9 {
            warmup_retained_bytes = serde_json::to_vec(
                &harness
                    .sessions
                    .get(AgentSessionGetRequest {
                        session_id: session.session_id.clone(),
                    })
                    .unwrap(),
            )
            .unwrap()
            .len() as u64;
        }
    }
    let final_session = harness
        .sessions
        .get(AgentSessionGetRequest {
            session_id: session.session_id,
        })
        .unwrap();
    assert_eq!(final_session.turn_count, 100);
    let retained_bytes = serde_json::to_vec(&final_session).unwrap().len() as u64;
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let simulated_ns = u128::from(latency.micros()) * 1_000 * 100;
    Sample {
        elapsed_ns,
        simulated_wall_ns: simulated_ns,
        simulated_work_ns: simulated_ns,
        tasks: counts.tasks,
        continuations: counts.continuations,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes,
        post_warmup_growth_bytes: retained_bytes.saturating_sub(warmup_retained_bytes),
        output: json!({
            "turn_count": final_session.turn_count,
            "message_count": final_session.messages.len(),
            "retained_bytes": retained_bytes,
            "last_hash": canonical_hash(&last)
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn context_sample(label: &str, bytes: usize) -> Sample {
    let builder = ContextBuilder::default();
    builder.set_system_prompt("benchmark-v1-system");
    builder.set_tools(vec![benchmark_tool_descriptor()]);
    let content = "x".repeat(bytes);
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let context = builder
        .build(AgentContextBuildRequest {
            profile_id: "benchmark.profile".into(),
            messages: vec![AgentMessage::user(content)],
            session_id: None,
            max_context_tokens: Some(bytes as u64),
            metadata: Some(json!({"fixture": label, "seed": BENCHMARK_FIXED_SEED})),
        })
        .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    let retained_bytes = serde_json::to_vec(&context).unwrap().len() as u64;
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: 0,
        continuations: 0,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes,
        post_warmup_growth_bytes: 0,
        output: json!({
            "profile_id": context.profile_id,
            "message_bytes": bytes,
            "tools": context.tools.len(),
            "retained_bytes": retained_bytes
        }),
        allocations,
        allocated_bytes,
    }
}

#[derive(Default)]
struct BenchmarkLspFactory;

impl LspProcessFactory for BenchmarkLspFactory {
    fn spawn(&self, _descriptor: &LspServerDescriptor) -> Result<Box<dyn LspProcess>, AgentError> {
        Ok(Box::new(BenchmarkLspProcess::default()))
    }
}

#[derive(Default)]
struct BenchmarkLspProcess {
    pending: VecDeque<Value>,
}

impl LspProcess for BenchmarkLspProcess {
    fn send(&mut self, message: &Value) -> Result<(), AgentError> {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return Ok(());
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = match method {
            "textDocument/hover" => {
                json!({"contents": {"kind": "markdown", "value": "`fn target() -> u32`"}})
            }
            _ => Value::Null,
        };
        self.pending
            .push_back(json!({"jsonrpc": "2.0", "id": id, "result": result}));
        Ok(())
    }

    fn receive(&mut self, _timeout: Duration) -> Result<Option<Value>, AgentError> {
        Ok(self.pending.pop_front())
    }

    fn is_alive(&mut self) -> Result<bool, AgentError> {
        Ok(true)
    }

    fn terminate(&mut self) -> Result<(), AgentError> {
        Ok(())
    }
}

pub fn lsp_query_sample(query_count: usize) -> Sample {
    let workspace = LspWorkspaceId("benchmark".into());
    let document = LspDocumentId {
        workspace: workspace.clone(),
        uri: "file:///benchmark/main.rs".into(),
    };
    let service = SharedLspService::new(Arc::new(BenchmarkLspFactory));
    service
        .open_workspace(
            workspace.clone(),
            LspServerDescriptor {
                server_id: "benchmark".into(),
                command: "in-memory".into(),
                args: Vec::new(),
                workspace_uri: "file:///benchmark".into(),
                initialization_options: None,
            },
        )
        .unwrap();
    service
        .open_document(LspDocumentSnapshot {
            document: document.clone(),
            language_id: "rust".into(),
            version: 1,
            text: "fn target() -> u32 { 1 }".into(),
        })
        .unwrap();
    service
        .hover(
            &document,
            LspPosition {
                line: 0,
                character: 4,
            },
        )
        .unwrap();

    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = None;
    for _ in 0..query_count {
        last = Some(
            service
                .hover(
                    &document,
                    LspPosition {
                        line: 0,
                        character: 4,
                    },
                )
                .unwrap(),
        );
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let last = last.expect("LSP benchmark executes at least one query");
    let retained_bytes = serde_json::to_vec(&last).unwrap().len() as u64;
    service.close_workspace(&workspace).unwrap();
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: query_count as u64,
        continuations: 0,
        tool_routes: query_count as u64,
        max_tool_inflight: 1,
        retained_bytes,
        post_warmup_growth_bytes: 0,
        output: json!({
            "queries": query_count,
            "summary": last.summary,
            "inline": last.inline,
            "details": last.details,
            "open_workspaces_after_close": service.active_workspace_count(),
        }),
        allocations,
        allocated_bytes,
    }
}

#[derive(Default)]
struct BenchmarkAgentService;

impl InProcessAgentService for BenchmarkAgentService {
    fn dispatch(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let response = match request.request {
            AgentWireRequest::ListRuntimeCapabilities => {
                AgentWireResponse::Capabilities(BTreeMap::from([
                    ("event-resume".into(), "1".into()),
                    ("resource-ref".into(), "1".into()),
                ]))
            }
            _ => {
                return Err(AgentWireError {
                    code: "agent.benchmark.unsupported".into(),
                    message: "benchmark service only supports capabilities".into(),
                    retryable: false,
                });
            }
        };
        Ok(AgentWireResponseEnvelope {
            request_id: request.request_id,
            response: Ok(response),
        })
    }
}

pub fn client_link_sample(query_count: usize) -> Sample {
    let (client_connection, server_connection) = memory_transport_pair(
        EndpointId::from_bytes([0x31; 16]),
        EndpointId::from_bytes([0x32; 16]),
        MemoryTransportConfig::default(),
    );
    let server = thread::spawn(move || {
        let mut server = AgentLinkServer::new(server_connection, BenchmarkAgentService);
        let mut handled = 0;
        while handled < query_count + 1 {
            if server.serve_once().unwrap() {
                handled += 1;
            } else {
                thread::yield_now();
            }
        }
    });
    let backend =
        AgentLinkClient::new(client_connection).with_response_timeout(Duration::from_secs(2));
    let mut client = AgentClient::new(backend);
    client.negotiate().unwrap();

    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = BTreeMap::new();
    for _ in 0..query_count {
        last = client.runtime_capabilities().unwrap();
    }
    let elapsed_ns = started.elapsed().as_nanos();
    server.join().unwrap();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let retained_bytes = serde_json::to_vec(&last).unwrap().len() as u64;
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: query_count as u64,
        continuations: 0,
        tool_routes: 0,
        max_tool_inflight: 1,
        retained_bytes,
        post_warmup_growth_bytes: 0,
        output: json!({
            "queries": query_count,
            "capabilities": last,
        }),
        allocations,
        allocated_bytes,
    }
}

struct BenchmarkCapabilityResolver;

impl AgentCapabilityResolver for BenchmarkCapabilityResolver {
    fn resolve(&self, _capabilities: &[String]) -> Result<CapabilityBits, String> {
        Ok(CapabilityBits::default())
    }
}

pub fn distributed_placement_sample(placement_count: usize) -> Sample {
    let placement = AgentTaskPlacement {
        required_capabilities: Vec::new(),
        affinity: AgentAffinity::Required("benchmark-worker".into()),
        data_locality: Vec::new(),
        latency_class: "interactive".into(),
        cost_class: "standard".into(),
        remote_execution_allowed: true,
        migration: AgentMigrationPolicy::ReconcileIdempotent,
        side_effect: AgentSideEffectClass::Pure,
        required_resource_refs: Vec::new(),
    };
    let resolver = BenchmarkCapabilityResolver;
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = None;
    for index in 0..placement_count {
        last = Some(
            distributed_request(
                format!("placement-{index}"),
                "mutsuki.agent/run@1",
                &placement,
                &resolver,
                NodeId("benchmark-local".into()),
                64 * 1024,
                8 * 1024,
                1.0,
            )
            .unwrap(),
        );
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let last = last.expect("distributed placement benchmark executes at least once");
    let retained_bytes = serde_json::to_vec(&last).unwrap().len() as u64;
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: placement_count as u64,
        continuations: 0,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes,
        post_warmup_growth_bytes: 0,
        output: serde_json::to_value(last).unwrap(),
        allocations,
        allocated_bytes,
    }
}

pub fn checkpoint_codec_sample(roundtrips: usize) -> Sample {
    let resource = benchmark_session_resource();
    let checkpoint = AgentSessionCheckpoint {
        session_id: "benchmark-session".into(),
        profile_id: "coding".into(),
        version: SessionVersion(42),
        budget: AgentBudget {
            max_steps: Some(128),
            max_total_tokens: Some(1_000_000),
            max_cost_microunits: Some(10_000_000),
            deadline_unix_ms: None,
        },
        state: resource.clone(),
        snapshot: SessionSnapshotRef {
            session_id: "benchmark-session".into(),
            version: SessionVersion(42),
            snapshot: resource,
            base: None,
            deltas: Vec::new(),
        },
        pending_approvals: Vec::new(),
        plugin_generations: BTreeMap::from([
            ("lsp".into(), 7),
            ("model".into(), 11),
            ("tools".into(), 13),
        ]),
        attempts: BTreeMap::from([(
            "attempt-42".into(),
            AgentAttemptCheckpoint {
                attempt_id: "attempt-42".into(),
                turn_id: "turn-42".into(),
                step_index: 8,
                state: "waiting_model".into(),
                committed_side_effects: vec!["edit-plan-41".into()],
            },
        )]),
        coordinator: Some(CoordinatorLease {
            session_id: "benchmark-session".into(),
            node_id: "benchmark-worker".into(),
            epoch: 4,
            fencing_token: "benchmark-fence".into(),
            expires_at_unix_ms: u64::MAX,
        }),
        degraded_reason: None,
    };
    let encoded = serde_json::to_vec(&checkpoint).unwrap();
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut decoded = None;
    for _ in 0..roundtrips {
        let value: AgentSessionCheckpoint = serde_json::from_slice(&encoded).unwrap();
        let reencoded = serde_json::to_vec(&value).unwrap();
        assert_eq!(reencoded, encoded);
        decoded = Some(value);
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let decoded = decoded.expect("checkpoint benchmark executes at least once");
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: roundtrips as u64,
        continuations: 0,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes: encoded.len() as u64,
        post_warmup_growth_bytes: 0,
        output: serde_json::to_value(decoded).unwrap(),
        allocations,
        allocated_bytes,
    }
}

fn benchmark_session_resource() -> ResourceRef {
    ResourceRef {
        ref_id: "resource:benchmark-session:42".into(),
        resource_id: ResourceId {
            kind_id: "agent.session".into(),
            slot_id: "benchmark-session".into(),
            generation: 1,
            version: 42,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: "agent.session-store".into(),
        resource_kind: "agent.session".into(),
        schema: "mutsuki.agent.session@1".into(),
        version: 42,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: Some(64 * 1024),
        content_hash: Some("sha256:benchmark-session".into()),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

pub fn memory_route_sample() -> Sample {
    let router = MemoryRouter::default();
    for index in 0..128 {
        router
            .write(AgentMemoryWriteRequest {
                text: format!("benchmark candidate {index:03} rust agent memory"),
                tags: vec![if index % 2 == 0 { "even" } else { "odd" }.into()],
                metadata: Some(json!({"seed": BENCHMARK_FIXED_SEED, "index": index})),
            })
            .unwrap();
    }
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let result = router
        .query(AgentMemoryQueryRequest {
            query: "rust agent".into(),
            limit: 8,
            tags: vec!["even".into()],
        })
        .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(result.records.len(), 8);
    let retained_bytes = serde_json::to_vec(&result).unwrap().len() as u64;
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: 0,
        continuations: 0,
        tool_routes: 0,
        max_tool_inflight: 0,
        retained_bytes,
        post_warmup_growth_bytes: 0,
        output: serde_json::to_value(result).unwrap(),
        allocations,
        allocated_bytes,
    }
}

fn run_request(scenario: &str, session_id: Option<String>, max_steps: u32) -> AgentRunRequest {
    let mut request = AgentRunRequest::new(
        "benchmark.profile",
        vec![AgentMessage::user("benchmark fixed prompt")],
    );
    request.session_id = session_id;
    request.max_steps = max_steps;
    request.model = Some(BENCHMARK_MODEL_ID.into());
    request.metadata = Some(json!({
        "scenario": scenario,
        "failure": "none",
        "seed": BENCHMARK_FIXED_SEED
    }));
    request
}

fn failure_request(failure: &str) -> AgentRunRequest {
    let mut request = run_request("single-turn", None, 1);
    request.metadata = Some(json!({
        "scenario": "single-turn",
        "failure": failure,
        "seed": BENCHMARK_FIXED_SEED
    }));
    request
}

fn model_request(scenario: &str, tool_messages: Vec<AgentMessage>) -> AgentModelGenerateRequest {
    let mut messages = vec![AgentMessage::user("benchmark fixed prompt")];
    messages.extend(tool_messages);
    AgentModelGenerateRequest {
        model: BENCHMARK_MODEL_ID.into(),
        messages,
        temperature: None,
        max_output_tokens: None,
        provider_hint: None,
        metadata: Some(json!({"scenario": scenario, "failure": "none"})),
        result_protocol_id: None,
        result_context: None,
        session_id: None,
    }
}

fn immediate_outcome(runner: &mut dyn Runner, task: Task, runner_id: &str) -> TaskOutcome {
    let task_id = task.task_id.clone();
    outcome_from_completion(
        runner
            .run_batch(
                context(&task_id),
                batch(runner_id, std::slice::from_ref(&task)),
            )
            .unwrap(),
    )
}

fn outcome_from_completion(completion: CompletionBatch) -> TaskOutcome {
    let entry = completion.results.into_iter().next().unwrap();
    if let Some(error) = entry.error {
        return TaskOutcome::Failed {
            task_id: entry.task_id,
            error,
        };
    }
    let result = entry.result.unwrap();
    TaskOutcome::Completed {
        task_id: entry.task_id,
        output: result.output,
        output_ref: None,
    }
}

fn single_result(completion: CompletionBatch) -> Result<RunnerResult, RuntimeError> {
    let entry = completion.results.into_iter().next().unwrap();
    match (entry.result, entry.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(error),
        _ => Err(RuntimeError::new(
            "agent.benchmark.invalid_completion",
            "agent.benchmark",
            entry.task_id,
        )),
    }
}

fn context(id: &str) -> RunnerContext {
    let mut context = RunnerContext::new(1, 1, "agent-benchmark", Vec::<String>::new(), id)
        .with_batch(format!("batch:{id}"), 1);
    context.invocation_id = format!("invocation:{id}");
    context
}

fn batch(runner_id: &str, tasks: &[Task]) -> WorkBatch {
    WorkBatch {
        batch_id: format!("batch:{}", tasks[0].task_id),
        tick_id: "tick:agent-benchmark".into(),
        batch_key: runner_id.into(),
        entries: tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: task.task_id.clone(),
                task_id: task.task_id.clone(),
                trace_id: task.trace_id.clone(),
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(index),
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            })
            .collect(),
        payload: BatchPayload::from_task_refs(tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: Vec::new(),
    }
}
