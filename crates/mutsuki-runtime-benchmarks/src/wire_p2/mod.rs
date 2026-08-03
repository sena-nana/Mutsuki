mod abi;
mod codec;
#[allow(dead_code)]
#[path = "../wire/fixtures.rs"]
mod fixtures;
#[path = "../wire_p1/io.rs"]
mod io;
mod server;
mod transport;

use std::env;
use std::path::PathBuf;

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, CaseResult, GateResult};

pub fn run(allocator: &TrackingAllocator) -> Result<(), String> {
    let (mode, output, command) = options()?;
    let mut cases = codec::run(mode, allocator)?;
    cases.extend(transport::run(mode, allocator)?);
    cases.extend(abi::run(mode, allocator)?);
    let gates = gates(&cases);
    let report = crate::wire_report::write(
        "mutsuki-core-wire-p2/v2",
        "runtime-wire-p2/v1",
        mode,
        command,
        &output,
        cases,
        gates,
        "Runtime Wire binary transport and ABI diagnostic with tracking allocator; not headline latency",
    )?;
    let passed = report.correctness.passed;
    println!(
        "Epic #30 P2 benchmark: {} cases, {} gates, result={}, report={}",
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

fn gates(cases: &[CaseResult]) -> Vec<GateResult> {
    let _ = cases;
    Vec::new()
}

fn options() -> Result<(BenchmarkMode, PathBuf, String), String> {
    let args = env::args().collect::<Vec<_>>();
    let mode = match args.get(1).map(String::as_str) {
        Some("smoke") => BenchmarkMode::Smoke,
        Some("full") => BenchmarkMode::Full,
        _ => return Err("usage: runtime_wire_p2 <smoke|full> [--output path]".into()),
    };
    let mut output = PathBuf::from(format!(
        "target/mutsuki-benchmarks/issue30-p2-{}.json",
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
