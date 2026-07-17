mod rejection;

use std::env;
use std::path::PathBuf;

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, GateResult};

pub fn run(allocator: &TrackingAllocator) -> Result<(), String> {
    let (mode, output, command) = options()?;
    let cases = rejection::run(mode, allocator)?;
    let gates = cases
        .iter()
        .map(|case| GateResult {
            gate_id: format!("p3.{}.bounded-latency", case.id.replace('/', ".")),
            kind: "bounded_rejection".into(),
            passed: case.ns_per_unit <= 50_000.0 && case.counters["rejected"] == case.units as i128,
            actual: case.ns_per_unit,
            limit: 50_000.0,
            unit: "ns/frame".into(),
        })
        .collect::<Vec<_>>();
    let report = crate::wire_report::write(
        "mutsuki-core-wire-p3/v2",
        "runtime-wire-p3/v1",
        mode,
        command,
        &output,
        cases,
        gates,
        "Runtime Wire hostile-input rejection diagnostic with tracking allocator; not headline latency",
    )?;
    let passed = report.correctness.passed;
    println!(
        "Epic #30 P3 rejection benchmark: {} cases, {} gates, result={}, report={}",
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
        _ => return Err("usage: runtime_wire_p3 <smoke|full> [--output path]".into()),
    };
    let mut output = PathBuf::from(format!(
        "target/mutsuki-benchmarks/issue30-p3-{}.json",
        mode.as_str()
    ));
    if args.get(2).map(String::as_str) == Some("--output") {
        output = PathBuf::from(
            args.get(3)
                .ok_or_else(|| "--output expects a path".to_string())?,
        );
    } else if args.len() > 2 {
        return Err(format!("unknown arguments: {}", args[2..].join(" ")));
    }
    Ok((mode, output, args.join(" ")))
}
