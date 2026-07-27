use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use mutsuki_bot_link_parser::{expand_card_payload, extract_urls};
use mutsuki_bot_protocol::*;
use mutsuki_bot_testkit::{
    BENCHMARK_FIXED_SEED, benchmark_card_payload, benchmark_event, benchmark_gateway_frame,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    GatewayAction, HttpMethod, QqBotConfig, QqGatewayPump, QqHttpClient, QqHttpRequest,
    QqHttpResponse, QqOpenApiError, QqOpenApiTransport, StaticQqCredentials,
};
use mutsuki_plugin_bot_command::BotCommandRunner;
use mutsuki_plugin_bot_event_router::{BOT_EVENT_ROUTER_RUNNER_ID, BotEventRouterRunner};
use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CompletionBatch, DispatchLane, OrderingRequirement, RunnerContext,
    RunnerResult, RunnerStatus, RuntimeError, Task, TaskBatch, TaskHandle, TaskOutcome,
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
    let mut router = BotEventRouterRunner::new(
        1,
        vec![BotEventSubscription::new(
            "benchmark-command",
            BOT_COMMAND_PARSE_PROTOCOL_ID,
        )],
    );
    let mut command = BotCommandRunner::new(1, vec!["/".into()]);
    let mut command_tasks = Vec::with_capacity(event_count);
    let mut adapters = BTreeMap::<String, u64>::new();
    for chunk in events.chunks(64) {
        for event in chunk {
            *adapters.entry(event.bot.account_id.clone()).or_default() += 1;
        }
        let tasks = chunk
            .iter()
            .map(|event| {
                Task::new(
                    format!("ingest:{}", event.event_id),
                    BOT_EVENT_INGEST_PROTOCOL_ID,
                    TaskPayload::from_local(event.clone()),
                )
            })
            .collect::<Vec<_>>();
        let completion = router
            .run_batch(
                context("router", tasks.len()),
                batch(BOT_EVENT_ROUTER_RUNNER_ID, &tasks),
            )
            .unwrap();
        command_tasks.extend(successful_tasks(completion));
    }
    let mut handler_tasks = Vec::new();
    for chunk in command_tasks.chunks(64) {
        let completion = command
            .run_batch(
                context("command", chunk.len()),
                batch(mutsuki_plugin_bot_command::BOT_COMMAND_RUNNER_ID, chunk),
            )
            .unwrap();
        handler_tasks.extend(successful_tasks(completion));
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    let minimum = adapters.values().copied().min().unwrap_or(0);
    let maximum = adapters.values().copied().max().unwrap_or(1);
    let fairness = minimum as f64 / maximum.max(1) as f64;
    assert_eq!(command_tasks.len(), event_count);
    assert_eq!(handler_tasks.len(), (event_count + 1) / 2);
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
            "commands": handler_tasks.len(),
            "adapter_counts": adapters,
            "first_handler": handler_tasks.first().map(|task| &task.task_id),
            "last_handler": handler_tasks.last().map(|task| &task.task_id)
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
        TaskPayload::from_local(event),
    );
    let mut runner = BotCommandRunner::new(1, vec!["/".into()]);
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let result = single_result(
        runner
            .run_batch(
                context("command-case", 1),
                batch(
                    mutsuki_plugin_bot_command::BOT_COMMAND_RUNNER_ID,
                    std::slice::from_ref(&task),
                ),
            )
            .unwrap(),
    )
    .unwrap();
    let elapsed_ns = started.elapsed().as_nanos();
    assert_eq!(!result.tasks.is_empty(), hit);
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
            "handler_task": result.tasks.first().map(|task| serde_json::to_value(task).unwrap())
        }),
        allocations,
        allocated_bytes,
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
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let mut accepted = 0_u64;
    for index in 0..event_count {
        if pump
            .handle_raw_frame(benchmark_gateway_frame(index, 4, false), 1)
            .unwrap()
            .is_some()
        {
            accepted += 1;
        }
        let _ = pump.pop_action();
    }
    let old = benchmark_gateway_frame(0, 4, false);
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
        .build();
    let mut runner = TaskAwaitRunnerAdapter::new(
        descriptor,
        client.clone(),
        Box::new(|ctx, task| {
            Box::pin(async move {
                let outcome = ctx
                    .call_raw("mutsuki.bot.benchmark/result@1", json!({"fixed": true}))
                    .await?;
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
    outcomes: Mutex<BTreeMap<String, TaskOutcome>>,
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

fn successful_tasks(completion: CompletionBatch) -> Vec<Task> {
    completion
        .results
        .into_iter()
        .flat_map(|entry| {
            assert!(entry.error.is_none());
            entry.result.unwrap().tasks
        })
        .collect()
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
    RunnerContext::new(1, 1, "bot-benchmark", Vec::<String>::new(), id)
        .with_batch(format!("batch:{id}"), entries)
}

fn batch(runner_id: &str, tasks: &[Task]) -> WorkBatch {
    WorkBatch {
        batch_id: format!("batch:{}", tasks[0].task_id),
        tick_id: "tick:bot-benchmark".into(),
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
                ordering: OrderingRequirement::PreserveSubmitOrder,
            })
            .collect(),
        payload: BatchPayload::from_task_refs(tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: Vec::new(),
    }
}
