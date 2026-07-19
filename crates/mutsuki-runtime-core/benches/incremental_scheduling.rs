use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use mutsuki_runtime_contracts::{ExecutionClass, RunnerDescriptor, RunnerPurity, Task};
use mutsuki_runtime_core::TaskPool;
use serde_json::{Value, json};

const RUNNER_COUNTS: [usize; 3] = [1, 16, 128];
const TASK_COUNTS: [usize; 3] = [1_000, 10_000, 100_000];
const IDLE_ITERATIONS: usize = 200;

fn main() {
    let mut cases = Vec::new();
    for runner_count in RUNNER_COUNTS {
        for task_count in TASK_COUNTS {
            cases.push(benchmark_case(runner_count, task_count));
        }
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "issue": 26,
            "profile": "release",
            "idle_iterations": IDLE_ITERATIONS,
            "cases": cases,
        }))
        .expect("benchmark report must serialize")
    );
}

fn benchmark_case(runner_count: usize, task_count: usize) -> Value {
    let runners = (0..runner_count).map(runner_descriptor).collect::<Vec<_>>();

    let mut idle_pool = TaskPool::default();
    for index in 0..task_count {
        let task_id = format!("idle-{index}");
        idle_pool
            .enqueue(Task::new(&task_id, "bench.idle", json!({"index": index})))
            .expect("idle benchmark task must enqueue");
        idle_pool
            .cancel_by_core(&task_id, 0)
            .expect("idle benchmark task must become terminal");
    }
    assert_eq!(idle_pool.ready_count(), 0);
    let idle_started = Instant::now();
    for step in 1..=IDLE_ITERATIONS as u64 {
        black_box(idle_pool.wake_due_tasks(step));
        for runner in &runners {
            black_box(idle_pool.runner_load(runner, step, 0));
        }
    }
    let idle_tick_ns = idle_started.elapsed().as_nanos() as f64 / IDLE_ITERATIONS as f64;

    let target_per_runner = (task_count / runner_count / 4).clamp(1, 8);
    let target_count = target_per_runner * runner_count;
    let noise_count = task_count.saturating_sub(target_count);
    let mut isolated_pool = TaskPool::default();
    for index in 0..noise_count {
        isolated_pool
            .enqueue(Task::new(
                format!("noise-{index}"),
                format!("bench.noise.{}", index % 64),
                json!({"index": index}),
            ))
            .expect("noise benchmark task must enqueue");
    }
    for runner_index in 0..runner_count {
        for target_index in 0..target_per_runner {
            isolated_pool
                .enqueue(Task::new(
                    format!("target-{runner_index}-{target_index}"),
                    format!("bench.target.{runner_index}"),
                    json!({"runner": runner_index, "target": target_index}),
                ))
                .expect("target benchmark task must enqueue");
        }
    }
    let load_started = Instant::now();
    let isolated_queued = runners
        .iter()
        .map(|runner| isolated_pool.runner_load(runner, 1, 0).queued_count)
        .sum::<usize>();
    let isolated_load_ns = load_started.elapsed().as_nanos();
    let claim_started = Instant::now();
    let mut claimed = 0usize;
    for round in 0..target_per_runner {
        for runner in &runners {
            claimed += isolated_pool
                .claim_ready_for_executor(runner, format!("bench-executor-{round}"), 1, 0, 1)
                .len();
        }
    }
    let isolated_claim_ns = claim_started.elapsed().as_nanos();
    assert_eq!(isolated_queued, target_count);
    assert_eq!(claimed, target_count);

    json!({
        "runner_count": runner_count,
        "task_count": task_count,
        "idle_tick_ns": idle_tick_ns,
        "isolated_target_count": target_count,
        "isolated_queued": isolated_queued,
        "isolated_load_ns": isolated_load_ns,
        "isolated_claimed": claimed,
        "isolated_claim_ns": isolated_claim_ns,
    })
}

fn runner_descriptor(index: usize) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: format!("bench.runner.{index}"),
        plugin_id: "bench.plugin".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![format!("bench.target.{index}")],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Cpu,
        invocation_mode: Default::default(),
        concurrency: Default::default(),
        input_schema: json!({}),
        output_schema: json!({}),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:bench.runner.{index}")],
    }
}
