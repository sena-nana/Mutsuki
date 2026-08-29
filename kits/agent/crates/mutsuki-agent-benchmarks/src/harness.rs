use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use mutsuki_agent_client::{AgentClient, AgentLinkClient, AgentLinkServer, InProcessAgentService};
use mutsuki_agent_contracts::*;
use mutsuki_agent_distributed_integration::{AgentCapabilityResolver, distributed_request};
use mutsuki_agent_plugin_code_index::SharedCodeIndexService;
use mutsuki_agent_plugin_computer_use::{
    FakeBrowserBackend, FakeProcessBackend, InMemoryFilesystemBackend, SharedComputerUseService,
};
use mutsuki_agent_plugin_lsp::{LspProcess, LspProcessFactory, SharedLspService};
use mutsuki_agent_plugin_next_edit::{NextEditServiceConfig, SharedNextEditService};
use mutsuki_agent_plugin_web_search::{
    FakeHttpTransport, HttpJsonSearchService, HttpPageFetchService, SharedWebSearchService,
};
use mutsuki_agent_runtime::{CredentialBrokerService, KnowledgeService, SkillRegistry, SkillRoots};
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
    BatchEntry, BatchPayload, CompletionBatch, DispatchLane, EntryId, OrderingRequirement,
    ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    RunnerContext, RunnerResult, RunnerStatus, RuntimeError, Task, TaskBatch, TaskHandle, TaskId,
    TaskLeaseId, TaskOutcome, WorkBatch, WorkResourcePlan,
};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{RuntimeClient, RuntimeResult};
use serde_json::{Value, json};

use crate::measurement::{Sample, allocation_delta, allocation_snapshot, canonical_hash};

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
                context: None,
                permission_mode: Default::default(),
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
                    context: None,
                    permission_mode: Default::default(),
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
                            context: None,
                            permission_mode: Default::default(),
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
            parts: Vec::new(),
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
            session_id: None,
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
            turn_id: None,
            max_context_tokens: Some(bytes as u64),
            compaction: None,
            metadata: Some(json!({"fixture": label, "seed": BENCHMARK_FIXED_SEED})),
            system_instructions: Vec::new(),
            prompt_fragments: Vec::new(),
            prompt_template_id: None,
            memory_query: None,
            providers: Vec::new(),
            knowledge: None,
            discover_skills: false,
            session_version: None,
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

pub fn web_search_sample(query_count: usize) -> Sample {
    let resources = mutsuki_agent_runtime::AgentResourceStore::default();
    let hits = vec![SearchHit {
        title: "Benchmark".into(),
        url: "https://docs.example.com/bench".into(),
        canonical_url: "https://docs.example.com/bench".into(),
        snippet: Some("bench".into()),
        published_at: None,
        score: Some(1.0),
        untrusted_content: true,
    }];
    let transport = Arc::new(
        FakeHttpTransport::default()
            .with_post("https://search.example/v1", 200, json!({"hits": hits}))
            .with_get(
                "https://docs.example.com/bench",
                200,
                "text/html",
                b"<html><head><title>Benchmark</title></head><body>ok</body></html>".to_vec(),
            ),
    );
    let search = HttpJsonSearchService::new(
        SearchProviderConfig {
            provider_id: "generic-json".into(),
            endpoint: "https://search.example/v1".into(),
            headers: Vec::new(),
            credential_env: None,
            timeout_ms: Some(1_000),
            enable_http: true,
            enable_browser_fallback: false,
        },
        transport.clone(),
        resources.clone(),
    )
    .unwrap();
    let pages = HttpPageFetchService::new(transport, resources.clone(), true, false, None);
    let service =
        SharedWebSearchService::new(Arc::new(search), Arc::new(pages), resources, true, false);
    service
        .search(SearchQuery {
            query: "warmup".into(),
            locale: None,
            time_range: None,
            allow_domains: Vec::new(),
            deny_domains: Vec::new(),
            limit: 3,
        })
        .unwrap();

    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = None;
    for index in 0..query_count {
        let result = service
            .search(SearchQuery {
                query: format!("benchmark-{index}"),
                locale: None,
                time_range: None,
                allow_domains: Vec::new(),
                deny_domains: Vec::new(),
                limit: 3,
            })
            .unwrap();
        last = Some(service.extract(PageFetchRequest {
            url: result.hits[0].url.clone(),
            follow_redirects: true,
            max_redirects: 1,
            max_bytes: 64 * 1024,
            timeout_ms: 1_000,
            allow_browser_fallback: false,
        }));
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let last = last
        .expect("web search benchmark executes at least one query")
        .unwrap();
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: query_count as u64,
        continuations: 0,
        tool_routes: (query_count * 2) as u64,
        max_tool_inflight: 1,
        retained_bytes: serde_json::to_vec(&last).unwrap().len() as u64,
        post_warmup_growth_bytes: 0,
        output: json!({
            "queries": query_count,
            "title": last.title,
            "canonical_url": last.canonical_url,
            "untrusted": last.untrusted_content,
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn computer_use_sample(op_count: usize) -> Sample {
    let resources = mutsuki_agent_runtime::AgentResourceStore::default();
    let fs = Arc::new(
        InMemoryFilesystemBackend::default()
            .with_file("src/lib.rs", b"pub fn answer() -> u32 { 42 }")
            .with_file("README.md", b"computer use benchmark"),
    );
    let service = SharedComputerUseService::new(
        fs,
        Some(Arc::new(FakeProcessBackend::default())),
        Some(Arc::new(FakeBrowserBackend::default())),
        resources,
    );
    let workspace = AgentWorkspaceRef {
        workspace_id: "benchmark".into(),
        root: "/virtual".into(),
    };
    service
        .call_value(json!({
            "op": "grep",
            "workspace": workspace,
            "pattern": "answer"
        }))
        .unwrap();

    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = Value::Null;
    for index in 0..op_count {
        last = service
            .call_value(json!({
                "op": "read",
                "request": {"workspace": workspace, "path": "src/lib.rs"},
                "max_bytes": 4096
            }))
            .unwrap();
        if index % 4 == 0 {
            last = service
                .call_value(json!({
                    "op": "patch",
                    "request": {
                        "workspace": workspace,
                        "path": "src/lib.rs",
                        "old_text": "42",
                        "new_text": "42"
                    },
                    "session_id": "bench",
                    "turn_id": format!("t{index}"),
                    "approved": true
                }))
                .unwrap();
        }
        if index % 8 == 0 {
            let _ = service
                .call_value(json!({
                    "op": "exec",
                    "request": {
                        "workspace": workspace,
                        "command": "echo",
                        "args": ["ok"],
                        "limits": ExecutionLimits::default(),
                        "allow_network": false
                    },
                    "session_id": "bench",
                    "turn_id": format!("t{index}"),
                    "approved": true
                }))
                .unwrap();
            last = service
                .call_value(json!({
                    "op": "browser_snapshot",
                    "request": {"url": "https://example.com", "limits": ExecutionLimits::default()},
                    "session_id": "bench",
                    "turn_id": format!("t{index}"),
                    "approved": true
                }))
                .unwrap();
        }
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: op_count as u64,
        continuations: 0,
        tool_routes: op_count as u64,
        max_tool_inflight: 1,
        retained_bytes: serde_json::to_vec(&last).unwrap().len() as u64,
        post_warmup_growth_bytes: 0,
        output: json!({
            "ops": op_count,
            "last": last,
            "active_handles": service.active_handle_count(),
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn code_index_sample(query_count: usize) -> Sample {
    let resources = mutsuki_agent_runtime::AgentResourceStore::default();
    let service = SharedCodeIndexService::new(resources);
    let workspace = CodeWorkspaceRef {
        workspace_id: "benchmark".into(),
        root: "/benchmark".into(),
        tenant_id: "bench".into(),
        git_revision: Some("deadbeef".into()),
        worktree_id: None,
    };
    service
        .open_workspace(workspace.clone(), None, None, false)
        .unwrap();
    let mut changes = Vec::new();
    for index in 0..32 {
        changes.push(CodeFileChange::Create {
            path: format!("src/mod{index}.rs"),
            content: format!("pub fn work_{index}() -> u32 {{ {index} }}\npub struct Item{index};"),
        });
        changes.push(CodeFileChange::Create {
            path: format!("web/view{index}.ts"),
            content: format!("export function render_{index}() {{ return {index}; }}\n"),
        });
    }
    service
        .apply_batch(CodeIndexBatch {
            workspace: workspace.clone(),
            rebuild: false,
            changes,
        })
        .unwrap();

    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = None;
    for index in 0..query_count {
        last = Some(
            service
                .search(CodeSearchQuery {
                    workspace: workspace.clone(),
                    query: format!("work_{}", index % 16),
                    mode: CodeSearchMode::Symbol,
                    path_prefix: None,
                    limit: 8,
                    include_overlay: false,
                })
                .unwrap(),
        );
        if index % 4 == 0 {
            last = Some(
                service
                    .search(CodeSearchQuery {
                        workspace: workspace.clone(),
                        query: "render_".into(),
                        mode: CodeSearchMode::Text,
                        path_prefix: Some("web/".into()),
                        limit: 8,
                        include_overlay: false,
                    })
                    .unwrap(),
            );
        }
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let last = last.expect("code index benchmark executes at least one query");
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: query_count as u64,
        continuations: 0,
        tool_routes: query_count as u64,
        max_tool_inflight: 1,
        retained_bytes: serde_json::to_vec(&last).unwrap().len() as u64,
        post_warmup_growth_bytes: 0,
        output: json!({
            "queries": query_count,
            "hits": last.hits.len(),
            "revision": last.index_revision,
            "active_workspaces": service.active_workspace_count(),
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn next_edit_sample(plan_count: usize) -> Sample {
    let resources = mutsuki_agent_runtime::AgentResourceStore::default();
    let service = SharedNextEditService::with_config(
        resources,
        NextEditServiceConfig {
            debounce_ms: 0,
            ..NextEditServiceConfig::default()
        },
    );
    let workspace = EditorWorkspaceRef {
        workspace_id: "benchmark".into(),
        folders: vec!["/workspace".into()],
        metadata: json!({}),
    };
    let document = EditorDocumentRef {
        workspace_id: "benchmark".into(),
        uri: "file:///workspace/main.rs".into(),
    };

    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = Value::Null;
    for index in 0..plan_count {
        let request = NextEditRequest {
            request_id: format!("bench-{index}"),
            workspace: workspace.clone(),
            generation: (index + 1) as u64,
            editor_generation: (index + 1) as u64,
            document_versions: vec![(document.clone(), DocumentVersion((index % 8) as u64 + 1))],
            document_contexts: Vec::new(),
            recent_edits: Vec::new(),
            diagnostics: vec![NextEditDiagnosticHint {
                document: document.clone(),
                diagnostic: LspDiagnostic {
                    range: LspRange {
                        start: LspPosition {
                            line: (index % 20) as u32,
                            character: 0,
                        },
                        end: LspPosition {
                            line: (index % 20) as u32,
                            character: 4,
                        },
                    },
                    severity: Some(1),
                    code: None,
                    message: format!("diag-{index}"),
                },
            }],
            related_paths: Vec::new(),
            git_diff: if index % 5 == 0 {
                vec![NextEditDiffHint {
                    path: "main.rs".into(),
                    summary: format!("diff-{index}"),
                    details: None,
                }]
            } else {
                Vec::new()
            },
            expected_git_head: Some(GitHeadIdentity {
                commit: "deadbeef".into(),
                branch: Some("main".into()),
                upstream: None,
                generation: 1,
            }),
            intent: None,
            path: NextEditPlanningPath::Lightweight,
            min_confidence: 0.55,
            allow_multi_file: false,
            deadline_unix_ms: Some(u64::MAX),
            now_unix_ms: 1_000 + index as u64,
            metadata: json!({}),
        };
        last = serde_json::to_value(
            service
                .call_typed(NextEditServiceRequest::Plan {
                    request: Box::new(request),
                })
                .unwrap(),
        )
        .unwrap();
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: plan_count as u64,
        continuations: 0,
        tool_routes: plan_count as u64,
        max_tool_inflight: 1,
        retained_bytes: serde_json::to_vec(&last).unwrap().len() as u64,
        post_warmup_growth_bytes: 0,
        output: json!({
            "plans": plan_count,
            "last": last,
        }),
        allocations,
        allocated_bytes,
    }
}

pub fn native_coding_bundle_sample(iterations: usize) -> Sample {
    use std::collections::BTreeMap;

    use mutsuki_agent_bundle::{
        EchoChildExecutor, NativeCodingAgentBundle, NativeCodingBackends, run_fix_golden_path,
        run_review_golden_path,
    };
    use mutsuki_agent_plugin_computer_use::InMemoryFilesystemBackend;
    use mutsuki_agent_plugin_git::InMemoryGitBackend;

    let mut files = BTreeMap::new();
    files.insert(
        "src/lib.rs".into(),
        "pub fn answer() -> u32 { 41 }\n".into(),
    );
    let git = Arc::new(InMemoryGitBackend::default().seed_repo("/workspace", files));
    let fs = Arc::new(
        InMemoryFilesystemBackend::default()
            .with_file("src/lib.rs", b"pub fn answer() -> u32 { 41 }\n"),
    );
    let bundle = NativeCodingAgentBundle::reference_with_child_executor(
        NativeCodingBackends {
            git,
            filesystem: fs,
            ..Default::default()
        }
        .with_fake_process_and_browser(),
        Arc::new(EchoChildExecutor),
    );
    bundle.assert_shared_service_identity().unwrap();
    bundle.assert_no_official_agent_server_dependency().unwrap();

    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut last = Value::Null;
    for _ in 0..iterations {
        last = run_fix_golden_path(&bundle).unwrap();
        let _ = run_review_golden_path(&bundle).unwrap();
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        simulated_wall_ns: 0,
        simulated_work_ns: 0,
        tasks: (iterations * 2) as u64,
        continuations: 0,
        tool_routes: (iterations * 2) as u64,
        max_tool_inflight: 1,
        retained_bytes: serde_json::to_vec(&last).unwrap().len() as u64,
        post_warmup_growth_bytes: 0,
        output: json!({
            "iterations": iterations,
            "last": last,
            "providers": bundle.profile.providers.len(),
            "official_servers": 0,
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

pub fn skill_discover_sample(skill_count: usize) -> Sample {
    let workspace = tempfile::tempdir().expect("skill benchmark tempdir");
    for index in 0..skill_count {
        let dir = workspace.path().join(format!("skill-{index:03}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nid: skill-{index:03}\nversion: 1.0.0\ntitle: Skill {index:03}\nsummary: benchmark skill {index:03}\n---\n\nBenchmark skill body {index:03} seed={BENCHMARK_FIXED_SEED}.\n"
            ),
        )
        .unwrap();
    }
    let registry = SkillRegistry::new(
        AgentSkillPolicy::default(),
        SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            ..Default::default()
        },
    );
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let result = registry.discover(SkillDiscoverRequest::default()).unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(result.catalog.len(), skill_count);
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

pub fn knowledge_retrieve_sample(documents: usize, top_k: usize) -> Sample {
    let service = KnowledgeService::default();
    for index in 0..documents {
        service
            .ingest(IngestionPlan {
                collection_id: "docs".into(),
                tenant_id: "tenant-a".into(),
                workspace_id: "ws-a".into(),
                document_id: format!("doc-{index:03}"),
                title: format!("Doc {index:03}"),
                content_type: KnowledgeContentType::Text,
                content: format!(
                    "benchmark knowledge document {index:03} rust agent retrieval seed={BENCHMARK_FIXED_SEED}"
                ),
                rebuild: false,
            })
            .unwrap();
    }
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let result = service
        .retrieve(RetrievalQuery {
            query: "rust agent retrieval".into(),
            tenant_id: "tenant-a".into(),
            workspace_id: "ws-a".into(),
            collection_ids: vec!["docs".into()],
            top_k,
            hybrid: true,
            rerank: false,
            max_excerpt_chars: None,
        })
        .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(result.citations.len(), top_k);
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

pub fn credential_resolve_sample(credentials: usize) -> Sample {
    let broker = CredentialBrokerService::default();
    let mut refs = Vec::with_capacity(credentials);
    for index in 0..credentials {
        let login = broker
            .login(CredentialLoginRequest {
                provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: format!("sk-benchmark-credential-{index:03}-abcdef0123456789"),
                account_label: None,
                source: Some("benchmark".into()),
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: json!({}),
            })
            .unwrap();
        refs.push(login.descriptor.credential);
    }
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut resolved = 0usize;
    for credential in &refs {
        let secret = broker.resolve_secret(credential).unwrap();
        assert!(secret.starts_with("sk-benchmark-credential-"));
        resolved += 1;
    }
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(resolved, credentials);
    let status = broker
        .status(CredentialStatusRequest {
            credential: refs[0].clone(),
        })
        .unwrap();
    let retained_bytes = serde_json::to_vec(&status.descriptor).unwrap().len() as u64;
    let encoded = serde_json::to_string(&status.event).unwrap();
    assert!(!encoded.contains("sk-benchmark-credential-"));
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
        output: json!({"resolved": resolved, "status": status.descriptor.status}),
        allocations,
        allocated_bytes,
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
                scope: None,
                priority: None,
                confidence: None,
                expiry_unix_ms: None,
                provenance: None,
                details_ref: None,
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
            scope: None,
            include_disabled: false,
            now_unix_ms: None,
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

fn context(id: impl AsRef<str>) -> RunnerContext {
    let id = id.as_ref();
    let mut context = RunnerContext::new(1, 1, "agent-benchmark", Vec::<TaskLeaseId>::new(), id)
        .with_batch(format!("batch:{id}"), 1);
    context.invocation_id = format!("invocation:{id}");
    context
}

fn batch(runner_id: &str, tasks: &[Task]) -> WorkBatch {
    WorkBatch {
        batch_id: format!("batch:{}", tasks[0].task_id).into(),
        tick_id: "tick:agent-benchmark".into(),
        batch_key: runner_id.into(),
        entries: tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: EntryId::from(task.task_id.as_str()),
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
