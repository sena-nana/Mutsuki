use std::collections::BTreeMap;
use std::hint::black_box;

use mutsuki_runtime_contracts::{
    ObservabilityOutletProfile, ObservabilityOverflowPolicy, RunnerResult, RuntimeEventKind,
    SpanStatus, Task, TraceSpan,
};
use mutsuki_runtime_core::{
    CoreRuntime, EventLog, Runner, TaskHistoryRetention, TaskPool, TraceLog,
};
use mutsuki_runtime_host::{NativeRunner, resolve_load_plan, runner_manifest};
use serde_json::json;

use crate::ALLOCATOR;
use crate::fixtures::{BENCH_PROTOCOL_ID, runner_descriptor, runtime_profile};
use crate::report::{BenchmarkMode, CaseResult};

pub fn run(mode: BenchmarkMode) -> Result<Vec<CaseResult>, String> {
    let mut cases = Vec::new();
    cases.push(idle_tick_case(mode)?);
    cases.extend(observability_cases(mode)?);
    cases.push(task_lifecycle_case(mode)?);
    cases.push(deadline_cancel_case(mode)?);
    cases.push(reload_case(mode)?);
    Ok(cases)
}

fn idle_tick_case(mode: BenchmarkMode) -> Result<CaseResult, String> {
    let iterations = mode.select(100_000, 8_640_000);
    let runner = runner_descriptor("bench.idle.runner", vec![BENCH_PROTOCOL_ID.into()], 1);
    let mut pool = TaskPool::default();
    let measurement = ALLOCATOR.measurement();
    for step in 1..=iterations {
        black_box(pool.wake_due_tasks(step));
        black_box(pool.runner_load(&runner, step, 0));
    }
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    if !pool.records().is_empty() || pool.ready_count() != 0 {
        return Err("idle tick mutated TaskPool".into());
    }
    Ok(CaseResult::measured(
        "longevity/idle-tick/24h-equivalent",
        "longevity",
        BTreeMap::from([
            ("tick_interval_ms".into(), "10".into()),
            (
                "equivalent_hours".into(),
                if mode == BenchmarkMode::Full {
                    "24"
                } else {
                    "0.278"
                }
                .into(),
            ),
        ]),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("retained_records".into(), pool.records().len() as i128),
            ("ready".into(), pool.ready_count() as i128),
        ]),
    ))
}

fn observability_cases(mode: BenchmarkMode) -> Result<Vec<CaseResult>, String> {
    let sustained = mode.select(50_000, 1_000_000);
    let enabled = mode.select(512, 4_096);
    Ok(vec![
        observability_case("disabled", sustained, 0)?,
        observability_case("enabled", enabled, enabled as usize + 1)?,
        observability_case("full-capacity", sustained, 64)?,
    ])
}

fn observability_case(state: &str, iterations: u64, capacity: usize) -> Result<CaseResult, String> {
    let profile =
        ObservabilityOutletProfile::new(capacity, ObservabilityOverflowPolicy::DropOldest);
    let mut events = EventLog::with_profile(profile.clone());
    let mut traces = TraceLog::with_profile(profile);
    let measurement = ALLOCATOR.measurement();
    for index in 0..iterations {
        events.record(
            RuntimeEventKind::Trace,
            "scheduler.decision",
            None,
            BTreeMap::new(),
            None,
        );
        black_box(traces.record_with(|sequence| TraceSpan {
            sequence,
            trace_id: format!("trace-{}", index % 8),
            span_id: format!("span-{sequence}"),
            parent_span_id: None,
            name: "scheduler.decision".into(),
            start: sequence as f64,
            end: Some(sequence as f64),
            attributes: BTreeMap::new(),
            status: SpanStatus::Ok,
        }));
    }
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    let expected_retained = capacity.min(iterations as usize);
    if events.retained() != expected_retained || traces.retained() != expected_retained {
        return Err(format!(
            "observability {state} retained events/traces {}/{}, expected {expected_retained}",
            events.retained(),
            traces.retained()
        ));
    }
    if capacity == 0 && traces.allocated_capacity() != 0 {
        return Err("disabled trace allocated persistent capacity".into());
    }
    Ok(CaseResult::measured(
        format!("longevity/observability/{state}"),
        "longevity",
        BTreeMap::from([
            ("state".into(), state.into()),
            ("capacity".into(), capacity.to_string()),
        ]),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("retained_events".into(), events.retained() as i128),
            ("dropped_events".into(), events.dropped() as i128),
            ("retained_traces".into(), traces.retained() as i128),
            ("dropped_traces".into(), traces.dropped() as i128),
            (
                "allocated_trace_capacity".into(),
                traces.allocated_capacity() as i128,
            ),
        ]),
    ))
}

fn task_lifecycle_case(mode: BenchmarkMode) -> Result<CaseResult, String> {
    let iterations = mode.select(20_000, 1_000_000);
    let runner = runner_descriptor("bench.lifecycle.runner", vec![BENCH_PROTOCOL_ID.into()], 1);
    let mut pool = TaskPool::default();
    pool.configure_history_retention(Some(TaskHistoryRetention::new(1_024, 2_048)));
    let warmup = 4_096_u64.min(iterations / 4);
    for index in 0..warmup {
        lifecycle_step(&mut pool, &runner, index)?;
    }
    let measurement = ALLOCATOR.measurement();
    let measured = iterations - warmup;
    let midpoint = warmup + measured / 2;
    let mut midpoint_bytes = ALLOCATOR.current_bytes();
    for index in warmup..iterations {
        lifecycle_step(&mut pool, &runner, index)?;
        if index + 1 == midpoint {
            midpoint_bytes = ALLOCATOR.current_bytes();
        }
    }
    let ending_bytes = ALLOCATOR.current_bytes();
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    if pool.retained_terminal_records() > 1_024 || pool.evicted_task_id_count() > 2_048 {
        return Err("terminal history retention exceeded configured bounds".into());
    }
    Ok(CaseResult::measured(
        "longevity/task-lifecycle/bounded-history",
        "longevity",
        BTreeMap::from([
            ("terminal_record_capacity".into(), "1024".into()),
            ("evicted_id_capacity".into(), "2048".into()),
        ]),
        measured,
        measured * 5,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("lifecycle_count".into(), iterations as i128),
            (
                "retained_terminal_records".into(),
                pool.retained_terminal_records() as i128,
            ),
            (
                "evicted_task_ids".into(),
                pool.evicted_task_id_count() as i128,
            ),
            (
                "terminal_records_evicted".into(),
                pool.statistics().terminal_records_evicted as i128,
            ),
            (
                "second_half_retained_growth_bytes".into(),
                ending_bytes as i128 - midpoint_bytes as i128,
            ),
        ]),
    ))
}

fn lifecycle_step(
    pool: &mut TaskPool,
    runner: &mutsuki_runtime_contracts::RunnerDescriptor,
    index: u64,
) -> Result<(), String> {
    let task_id = format!("lifecycle-{index}");
    pool.enqueue(Task::new(
        &task_id,
        BENCH_PROTOCOL_ID,
        json!({"index": index}),
    ))
    .map_err(|error| error.to_string())?;
    let first = pool
        .claim_ready_for_executor(runner, "lifecycle-executor", index, 0, 1)
        .pop()
        .ok_or_else(|| format!("lifecycle task {task_id} was not claimed"))?
        .0;
    pool.wait(&first, index, None)
        .map_err(|error| error.to_string())?;
    pool.wake(&task_id, index)
        .map_err(|error| error.to_string())?;
    let second = pool
        .claim_ready_for_executor(runner, "lifecycle-executor", index, 0, 1)
        .pop()
        .ok_or_else(|| format!("woken lifecycle task {task_id} was not reclaimed"))?
        .0;
    pool.complete(&second, index)
        .map_err(|error| error.to_string())
}

fn deadline_cancel_case(mode: BenchmarkMode) -> Result<CaseResult, String> {
    let iterations = mode.select(1_000, 10_000);
    let runner = runner_descriptor("bench.deadline.runner", vec![BENCH_PROTOCOL_ID.into()], 1);
    let mut pool = TaskPool::default();
    pool.configure_history_retention(Some(TaskHistoryRetention::new(256, 512)));
    let measurement = ALLOCATOR.measurement();
    let mut rejected = 0;
    for index in 0..iterations {
        let task_id = format!("deadline-{index}");
        pool.enqueue(Task::new(&task_id, BENCH_PROTOCOL_ID, json!({})))
            .map_err(|error| error.to_string())?;
        let lease = pool
            .claim_ready_for_executor_with_expiry(
                &runner,
                "deadline-executor",
                index,
                0,
                1,
                Some(index + 1),
            )
            .pop()
            .ok_or_else(|| format!("deadline task {task_id} was not claimed"))?
            .0;
        let failure = pool
            .complete(&lease, index + 1)
            .expect_err("completion at deadline must be fenced");
        if failure.error().code == mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT {
            rejected += 1;
        }
        pool.cancel_by_core(&task_id, index + 1)
            .map_err(|error| error.to_string())?;
    }
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    if rejected != iterations {
        return Err(format!(
            "deadline fencing rejected {rejected}/{iterations} stale completions"
        ));
    }
    Ok(CaseResult::measured(
        "longevity/deadline-cancel/cycles",
        "longevity",
        BTreeMap::new(),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("deadline_rejections".into(), rejected as i128),
            (
                "cancelled_retained".into(),
                pool.statistics().cancelled as i128,
            ),
        ]),
    ))
}

fn reload_case(mode: BenchmarkMode) -> Result<CaseResult, String> {
    let iterations = mode.select(50, 1_000);
    let descriptor = runner_descriptor("bench.reload.runner", vec![BENCH_PROTOCOL_ID.into()], 1);
    let manifest = runner_manifest(crate::fixtures::BENCH_PLUGIN_ID, vec![descriptor.clone()]);
    let profile = runtime_profile(Default::default());
    let base_plan = resolve_load_plan(&[manifest], &profile).map_err(|error| error.to_string())?;
    let mut runtime = CoreRuntime::boot(base_plan.clone(), vec![echo_runner(&descriptor)])
        .map_err(|error| error.to_string())?;
    let measurement = ALLOCATOR.measurement();
    for generation in 2..iterations + 2 {
        let mut plan = base_plan.clone();
        plan.registry_generation = generation;
        runtime
            .reload_with_runners(plan, vec![echo_runner(&descriptor)])
            .map_err(|error| error.to_string())?;
    }
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    if runtime.registry_snapshot().generation != iterations + 1
        || runtime.draining_generation_count() != 0
    {
        return Err("reload loop left an unexpected generation state".into());
    }
    Ok(CaseResult::measured(
        "longevity/reload/identical-surface",
        "longevity",
        BTreeMap::new(),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            (
                "final_generation".into(),
                runtime.registry_snapshot().generation as i128,
            ),
            (
                "draining_generations".into(),
                runtime.draining_generation_count() as i128,
            ),
        ]),
    ))
}

fn echo_runner(descriptor: &mutsuki_runtime_contracts::RunnerDescriptor) -> Box<dyn Runner> {
    Box::new(NativeRunner::new(descriptor.clone(), |_ctx, task| {
        Ok(RunnerResult::completed(task.task_id))
    }))
}
