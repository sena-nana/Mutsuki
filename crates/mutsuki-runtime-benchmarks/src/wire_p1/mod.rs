mod cases;
#[allow(dead_code)]
#[path = "../wire/fixtures.rs"]
mod fixtures;
mod io;
mod server;

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, GateResult};

pub fn run(allocator: &TrackingAllocator) -> Result<(), String> {
    let (mode, output, command) = options()?;
    let cases = cases::run(mode, allocator)?;
    let gates = performance_gates(&cases);
    let report = crate::wire_report::write(
        "mutsuki-core-wire-p1/v2",
        "runtime-wire-p1/v1",
        mode,
        command,
        &output,
        cases,
        gates,
        "Runtime Wire JSONL concurrency and management diagnostic with tracking allocator; not headline latency",
    )?;
    let passed = report.correctness.passed;
    println!(
        "Epic #30 P1 wire benchmark: {} cases, {} gates, result={}, report={}",
        report.cases.len(),
        report.gates.len(),
        if passed { "PASS" } else { "FAIL" },
        output.display()
    );
    passed.then_some(()).ok_or_else(|| {
        report
            .gates
            .iter()
            .filter(|gate| !gate.passed)
            .map(|gate| gate.gate_id.clone())
            .collect::<Vec<_>>()
            .join(", ")
    })
}

fn performance_gates(cases: &[crate::report::CaseResult]) -> Vec<GateResult> {
    let by_id = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let cancel = &by_id["wire/p1/jsonl/cancel-during-run_batch"];
    let cancel_p95 = cancel.counters["p95_ns"] as f64;
    let cancel_max = cancel.counters["max_ns"] as f64;
    let single = &by_id["wire/p1/jsonl/concurrent/in-flight-1"];
    let concurrent_16 = &by_id["wire/p1/jsonl/concurrent/in-flight-16"];
    let concurrent_56 = &by_id["wire/p1/jsonl/concurrent/in-flight-56"];
    let single_alloc = single.allocations.allocated_bytes as f64 / single.units as f64;
    let concurrent_56_alloc =
        concurrent_56.allocations.allocated_bytes as f64 / concurrent_56.units as f64;
    vec![
        GateResult {
            gate_id: "p1.cancel.p95".into(),
            kind: "management_latency".into(),
            passed: cancel_p95 <= 5_000_000.0,
            actual: cancel_p95,
            limit: 5_000_000.0,
            unit: "ns".into(),
        },
        GateResult {
            gate_id: "p1.cancel.max".into(),
            kind: "management_latency".into(),
            passed: cancel_max <= 20_000_000.0,
            actual: cancel_max,
            limit: 20_000_000.0,
            unit: "ns".into(),
        },
        GateResult {
            gate_id: "p1.concurrent-16.throughput-scaling".into(),
            kind: "optimization".into(),
            passed: concurrent_16.throughput_per_second >= single.throughput_per_second * 1.2,
            actual: concurrent_16.throughput_per_second,
            limit: single.throughput_per_second * 1.2,
            unit: "requests/s_min".into(),
        },
        GateResult {
            gate_id: "p1.concurrent-56.throughput-non-collapse".into(),
            kind: "non_regression".into(),
            passed: concurrent_56.throughput_per_second
                >= concurrent_16.throughput_per_second * 0.75,
            actual: concurrent_56.throughput_per_second,
            limit: concurrent_16.throughput_per_second * 0.75,
            unit: "requests/s_min".into(),
        },
        GateResult {
            gate_id: "p1.concurrent-56.allocated-bytes-per-request".into(),
            kind: "bounded_resource".into(),
            passed: concurrent_56_alloc <= single_alloc * 2.0,
            actual: concurrent_56_alloc,
            limit: single_alloc * 2.0,
            unit: "bytes/request".into(),
        },
    ]
}

fn options() -> Result<(BenchmarkMode, PathBuf, String), String> {
    let args = env::args().collect::<Vec<_>>();
    let mode = match args.get(1).map(String::as_str) {
        Some("smoke") => BenchmarkMode::Smoke,
        Some("full") => BenchmarkMode::Full,
        _ => return Err("usage: runtime_wire_p1 <smoke|full> [--output path]".into()),
    };
    let mut output = PathBuf::from(format!(
        "target/mutsuki-benchmarks/issue30-p1-{}.json",
        mode.as_str()
    ));
    let mut index = 2;
    while index < args.len() {
        if args[index] != "--output" {
            return Err(format!("unknown argument {}", args[index]));
        }
        index += 1;
        output = PathBuf::from(
            args.get(index)
                .ok_or_else(|| "--output expects a path".to_string())?,
        );
        index += 1;
    }
    Ok((mode, output, args.join(" ")))
}
