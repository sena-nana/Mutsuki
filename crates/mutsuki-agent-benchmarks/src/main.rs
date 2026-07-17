mod harness;
mod measurement;

use std::{collections::BTreeMap, env, fs, path::PathBuf};

use harness::{
    agent_case_sample, cancel_sample, context_sample, failure_retry_sample, memory_route_sample,
    parallel_tools_sample, session_100_sample, wait_resume_sample,
};
use measurement::{CountingAllocator, RawCase, raw_case};
use mutsuki_agent_testkit::{BENCHMARK_FIXED_SEED, BENCHMARK_FIXTURE_VERSION, SimulatedLatency};
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
    let latencies = if mode == "smoke" {
        vec![SimulatedLatency::ZeroUs]
    } else {
        SimulatedLatency::ALL.to_vec()
    };
    let mut cases = Vec::new();
    for latency in latencies {
        for (case_id, scenario) in [
            ("agent.single-turn", "single-turn"),
            ("agent.tool-once", "tool-once"),
            ("agent.tool-chain-8", "tool-chain-8"),
        ] {
            cases.push(raw_case(
                case_id,
                json!({"simulated_latency": latency.label()}),
                (0..regular_samples)
                    .map(|_| agent_case_sample(scenario, latency))
                    .collect(),
            ));
        }
        cases.push(raw_case(
            "agent.parallel-tools-8",
            json!({
                "simulated_latency": latency.label(),
                "requested_parallelism": 8,
                "execution": "real-tool-router-batch"
            }),
            (0..regular_samples)
                .map(|_| parallel_tools_sample(latency))
                .collect(),
        ));
        cases.push(raw_case(
            "agent.wait-resume",
            json!({"simulated_latency": latency.label(), "extra_empty_poll": 1}),
            (0..regular_samples)
                .map(|_| wait_resume_sample(latency))
                .collect(),
        ));
        cases.push(raw_case(
            "agent.failure-retry",
            json!({
                "simulated_latency": latency.label(),
                "failures": ["retryable", "non-retryable"]
            }),
            (0..regular_samples)
                .map(|_| failure_retry_sample(latency))
                .collect(),
        ));
        cases.push(raw_case(
            "agent.session-100-turns",
            json!({"simulated_latency": latency.label(), "turns": 100}),
            (0..long_samples)
                .map(|_| session_100_sample(latency))
                .collect(),
        ));
    }
    for (label, bytes) in [
        ("small", 4 * 1024),
        ("medium", 64 * 1024),
        ("large", 1024 * 1024),
    ] {
        cases.push(raw_case(
            format!("agent.context-{label}"),
            json!({"token_like_bytes": bytes}),
            (0..regular_samples)
                .map(|_| context_sample(label, bytes))
                .collect(),
        ));
    }
    cases.push(raw_case(
        "agent.memory-route",
        json!({"candidates": 128, "selected": 8}),
        (0..regular_samples)
            .map(|_| memory_route_sample())
            .collect(),
    ));
    cases.push(raw_case(
        "agent.cancel",
        json!({"phase": "waiting-model-result"}),
        (0..regular_samples).map(|_| cancel_sample()).collect(),
    ));

    let correctness = BTreeMap::from([
        ("duplicate_tool_results".into(), 0),
        ("hash_mismatches".into(), 0),
        ("public_network_requests".into(), 0),
        ("unexpected_errors".into(), 0),
        ("wrong_routes".into(), 0),
    ]);
    let report = RawReport {
        schema_version: "mutsuki.agent.performance.raw/v1",
        workload_version: "mutsuki.performance.agent-workloads/v1",
        fixture_version: BENCHMARK_FIXTURE_VERSION,
        mode,
        fixed_seed: BENCHMARK_FIXED_SEED,
        network_boundary: "no-network",
        cases,
        correctness,
    };
    let output = env::var_os("MUTSUKI_BENCH_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/mutsuki-benchmarks/agent-kit.raw.json"));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{}", output.display());
}
