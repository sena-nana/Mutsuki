mod cases;
mod fixtures;

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
        "mutsuki-core-wire-p0/v2",
        "runtime-wire-p0/v1",
        mode,
        command,
        &output,
        cases,
        gates,
        "Runtime Wire JSONL codec diagnostic with tracking allocator; not headline latency",
    )?;
    let passed = report.correctness.passed;
    println!(
        "Epic #30 P0 wire benchmark: {} cases, {} gates, result={}, report={}",
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

fn options() -> Result<(BenchmarkMode, PathBuf, String), String> {
    let args = env::args().collect::<Vec<_>>();
    let mode = match args.get(1).map(String::as_str) {
        Some("smoke") => BenchmarkMode::Smoke,
        Some("full") => BenchmarkMode::Full,
        _ => return Err("usage: runtime_wire <smoke|full> [--output path]".into()),
    };
    let mut output = PathBuf::from(format!(
        "target/mutsuki-benchmarks/issue30-p0-{}.json",
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

fn performance_gates(cases: &[crate::report::CaseResult]) -> Vec<GateResult> {
    let by_id = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let mut gates = Vec::new();
    for entries in [1, 16, 256, 4_096] {
        for direction in ["encode", "decode"] {
            let legacy =
                &by_id[format!("wire/p0/legacy_json_rpc/run_batch/{direction}/entries-{entries}")
                    .as_str()];
            let typed = &by_id
                [format!("wire/p0/typed_jsonl/run_batch/{direction}/entries-{entries}").as_str()];
            let legacy_bytes = legacy.allocations.allocated_bytes as f64;
            let typed_bytes = typed.allocations.allocated_bytes as f64;
            gates.push(GateResult {
                gate_id: format!("p0.{direction}.entries-{entries}.allocated-bytes"),
                kind: "optimization".into(),
                passed: typed_bytes < legacy_bytes,
                actual: typed_bytes,
                limit: legacy_bytes,
                unit: "bytes".into(),
            });
            gates.push(GateResult {
                gate_id: format!("p0.{direction}.entries-{entries}.latency"),
                kind: "non-regression".into(),
                passed: typed.ns_per_unit <= legacy.ns_per_unit * 1.15,
                actual: typed.ns_per_unit,
                limit: legacy.ns_per_unit * 1.15,
                unit: "ns/entry".into(),
            });
        }
    }
    gates
}
