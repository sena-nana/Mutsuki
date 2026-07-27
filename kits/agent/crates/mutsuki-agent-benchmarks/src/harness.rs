use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

use mutsuki_agent_protocol::*;
use mutsuki_agent_testkit::{
    BENCHMARK_FIXED_SEED, BENCHMARK_MODEL_ID, BENCHMARK_TOOL_NAME, BENCHMARK_TOOL_PROTOCOL,
    BenchmarkModelProvider, SimulatedLatency, benchmark_tool_descriptor, execute_benchmark_tool,
};
use mutsuki_plugin_agent_context::ContextBuilder;
use mutsuki_plugin_agent_loop::AgentLoop;
use mutsuki_plugin_agent_memory_router::MemoryRouter;
use mutsuki_plugin_agent_model_gateway::ModelGateway;
use mutsuki_plugin_agent_session::SessionStore;
use mutsuki_plugin_agent_tool_router::ToolRegistry;
use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CompletionBatch, DispatchLane, OrderingRequirement, RunnerContext,
    RunnerResult, RunnerStatus, RuntimeError, Task, TaskBatch, TaskHandle, TaskOutcome, WorkBatch,
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

    fn route_tool(&self, task: Task, counts: &mut RouteCounts) -> TaskOutcome {
        counts.tool_routes += 1;
        counts.max_tool_inflight = counts.max_tool_inflight.max(1);
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
