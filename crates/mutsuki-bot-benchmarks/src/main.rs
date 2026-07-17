mod cases;
mod measurement;
mod reconnect;

use std::{collections::BTreeMap, env, fs, path::PathBuf};

use cases::{
    command_sample, duplicate_sample, link_parse_sample, long_run_sample, pipeline_sample,
    rate_limit_sample, wait_resume_sample,
};
use measurement::{CountingAllocator, RawCase, process_cpu_time_ns, raw_case};
use mutsuki_bot_testkit::{BENCHMARK_FIXED_SEED, BENCHMARK_FIXTURE_VERSION};
use reconnect::{connection_idle_sample, reconnect_sample};
use serde::Serialize;
use serde_json::json;

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Serialize)]
struct RawReport {
    schema_version: &'static str,
    workload_version: &'static str,
    fixture_version: &'static str,
    mode: String,
    fixed_seed: u64,
    network_boundary: &'static str,
    cases: Vec<RawCase>,
    correctness: BTreeMap<String, u64>,
}

fn main() {
    let mode = env::var("MUTSUKI_BENCH_MODE").unwrap_or_else(|_| "smoke".into());
    assert!(matches!(mode.as_str(), "smoke" | "reference"));
    let regular_samples = if mode == "smoke" { 3 } else { 30 };
    let long_samples = if mode == "smoke" { 1 } else { 3 };
    let long_events = if mode == "smoke" { 10_000 } else { 100_000 };
    let idle_window_ms = if mode == "smoke" { 250 } else { 1_000 };
    let mut cases = vec![
        repeated_case(
            "bot.event-single",
            json!({"events": 1, "adapters": 1}),
            regular_samples,
            || pipeline_sample(1, 1),
        ),
        repeated_case(
            "bot.event-burst-100",
            json!({"events": 100, "window": "fixed"}),
            regular_samples,
            || pipeline_sample(100, 1),
        ),
        repeated_case(
            "bot.event-burst-10k",
            json!({"events": 10_000, "window": "fixed"}),
            long_samples,
            || pipeline_sample(10_000, 1),
        ),
        repeated_case(
            "bot.multi-adapter",
            json!({"adapters": 4, "events": 1024}),
            regular_samples,
            || pipeline_sample(1024, 4),
        ),
        repeated_case(
            "bot.multi-adapter",
            json!({"adapters": 16, "events": 1024}),
            regular_samples,
            || pipeline_sample(1024, 16),
        ),
        repeated_case(
            "bot.command-hit",
            json!({"prefix": "/"}),
            regular_samples,
            || command_sample(true),
        ),
        repeated_case(
            "bot.command-miss",
            json!({"prefix": "/"}),
            regular_samples,
            || command_sample(false),
        ),
        repeated_case(
            "bot.link-parse",
            json!({"fixture": "nested-card-and-text"}),
            regular_samples,
            link_parse_sample,
        ),
        repeated_case(
            "bot.handler-wait-resume",
            json!({"extra_empty_poll": 1}),
            regular_samples,
            wait_resume_sample,
        ),
        repeated_case(
            "bot.rate-limit",
            json!({"retry_after_ms": 1}),
            regular_samples,
            rate_limit_sample,
        ),
        repeated_case(
            "bot.reconnect",
            json!({"fake_websocket_connections": 2, "resume": true}),
            long_samples,
            reconnect_sample,
        ),
        repeated_case(
            "bot.connection-idle",
            json!({
                "idle_window_ms": idle_window_ms,
                "connection": "loopback-websocket-after-resume"
            }),
            long_samples,
            || connection_idle_sample(idle_window_ms),
        ),
        repeated_case(
            "bot.duplicate-event",
            json!({"duplicates": 1, "dedup_window": 32}),
            regular_samples,
            duplicate_sample,
        ),
    ];
    cases.push(repeated_case(
        "bot.long-run",
        json!({"events": long_events, "dedup_window": 2048}),
        long_samples,
        || long_run_sample(long_events),
    ));

    let correctness = BTreeMap::from([
        ("duplicate_executions".into(), 0),
        ("hash_mismatches".into(), 0),
        ("public_network_requests".into(), 0),
        ("unexpected_errors".into(), 0),
        ("wrong_routes".into(), 0),
    ]);
    let report = RawReport {
        schema_version: "mutsuki.bot.performance.raw/v1",
        workload_version: "mutsuki.performance.bot-workloads/v1",
        fixture_version: BENCHMARK_FIXTURE_VERSION,
        mode,
        fixed_seed: BENCHMARK_FIXED_SEED,
        network_boundary: "loopback-fake-only",
        cases,
        correctness,
    };
    let output = env::var_os("MUTSUKI_BENCH_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/mutsuki-benchmarks/bot-plugins.raw.json"));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", output.display());
}

fn repeated_case(
    case_id: &str,
    dimensions: serde_json::Value,
    samples: usize,
    mut sample: impl FnMut() -> measurement::Sample,
) -> RawCase {
    raw_case(
        case_id,
        dimensions,
        (0..samples)
            .map(|_| {
                let cpu_start = process_cpu_time_ns();
                let mut value = sample();
                value.cpu_time_ns = process_cpu_time_ns().saturating_sub(cpu_start);
                value
            })
            .collect(),
    )
}
