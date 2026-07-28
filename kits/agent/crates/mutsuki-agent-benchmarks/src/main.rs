mod harness;
mod measurement;

use std::{collections::BTreeMap, env, fs, path::PathBuf};

use harness::{
    agent_case_sample, cancel_sample, checkpoint_codec_sample, client_link_sample,
    code_index_sample, computer_use_sample, context_sample, credential_resolve_sample,
    distributed_placement_sample, failure_retry_sample, knowledge_retrieve_sample,
    lsp_query_sample, memory_route_sample, native_coding_bundle_sample, next_edit_sample,
    parallel_tools_sample, session_100_sample, skill_discover_sample, wait_resume_sample,
    web_search_sample,
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
        "agent.lsp-query-100",
        json!({"queries": 100, "transport": "in-memory-json-rpc"}),
        (0..regular_samples)
            .map(|_| lsp_query_sample(100))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.web-search-50",
        json!({"queries": 50, "transport": "injected-http-json"}),
        (0..regular_samples)
            .map(|_| web_search_sample(50))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.computer-use-50",
        json!({"ops": 50, "backends": ["in-memory-fs", "fake-process", "fake-browser"]}),
        (0..regular_samples)
            .map(|_| computer_use_sample(50))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.code-index-50",
        json!({"queries": 50, "files": 64, "modes": ["symbol", "text"]}),
        (0..regular_samples)
            .map(|_| code_index_sample(50))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.next-edit-50",
        json!({"plans": 50, "signals": ["diagnostic", "git_diff"]}),
        (0..regular_samples).map(|_| next_edit_sample(50)).collect(),
    ));
    cases.push(raw_case(
        "agent.client-link-query-100",
        json!({"queries": 100, "transport": "mutsuki-link-memory-control-stream"}),
        (0..regular_samples)
            .map(|_| client_link_sample(100))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.distributed-placement-100",
        json!({"placements": 100, "scheduler": "mutsuki-distributed"}),
        (0..regular_samples)
            .map(|_| distributed_placement_sample(100))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.checkpoint-codec-100",
        json!({"roundtrips": 100, "checkpoint": "full-durable-session"}),
        (0..regular_samples)
            .map(|_| checkpoint_codec_sample(100))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.memory-route",
        json!({"candidates": 128, "selected": 8}),
        (0..regular_samples)
            .map(|_| memory_route_sample())
            .collect(),
    ));
    cases.push(raw_case(
        "agent.skill-discover",
        json!({"skills": 32}),
        (0..regular_samples)
            .map(|_| skill_discover_sample(32))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.knowledge-retrieve",
        json!({"documents": 64, "top_k": 8}),
        (0..regular_samples)
            .map(|_| knowledge_retrieve_sample(64, 8))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.credential-resolve",
        json!({"credentials": 64, "resolves": 64}),
        (0..regular_samples)
            .map(|_| credential_resolve_sample(64))
            .collect(),
    ));
    cases.push(raw_case(
        "agent.native-coding-bundle",
        json!({
            "iterations": 8,
            "paths": ["fix", "review"],
            "providers": 2,
            "official_servers": 0
        }),
        (0..regular_samples)
            .map(|_| native_coding_bundle_sample(8))
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
