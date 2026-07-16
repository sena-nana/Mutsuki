mod abi;
mod codec;
#[allow(dead_code)]
#[path = "../wire/fixtures.rs"]
mod fixtures;
#[path = "../wire_p1/io.rs"]
mod io;
mod server;
mod transport;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::allocator::TrackingAllocator;
use crate::report::{BenchmarkMode, BenchmarkReport, CaseResult, Environment, GateResult};

pub fn run(allocator: &TrackingAllocator) -> Result<(), String> {
    let (mode, output, command) = options()?;
    let mut cases = codec::run(mode, allocator)?;
    cases.extend(transport::run(mode, allocator)?);
    cases.extend(abi::run(mode, allocator)?);
    let gates = gates(&cases);
    let passed = gates.iter().all(|gate| gate.passed);
    let report = BenchmarkReport {
        schema_version: 1,
        issue: 33,
        mode: mode.as_str().into(),
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs(),
        command,
        environment: environment(),
        cases,
        gates,
        passed,
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        &output,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| error.to_string())?;
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
            .map(|gate| gate.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    })
}

fn gates(cases: &[CaseResult]) -> Vec<GateResult> {
    let cases = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let mut gates = Vec::new();
    for entries in [16, 256, 4_096] {
        for direction in ["encode", "decode"] {
            let json = &cases[format!("wire/p2/jsonl/{direction}/entries-{entries}").as_str()];
            let binary = &cases[format!("wire/p2/binary/{direction}/entries-{entries}").as_str()];
            gates.push(gate(
                format!("p2.binary.{direction}.entries-{entries}.latency"),
                binary.ns_per_unit,
                json.ns_per_unit * 1.5,
                "ns/entry",
            ));
            if direction == "encode" {
                gates.push(gate(
                    format!("p2.binary.entries-{entries}.frame-bytes"),
                    binary.counters["frame_bytes"] as f64,
                    json.counters["frame_bytes"] as f64 - 1.0,
                    "bytes",
                ));
            }
        }
    }
    for surface in ["stdio", "native_abi"] {
        let json = &cases[format!("wire/p2/{surface}/jsonl").as_str()];
        let binary = &cases[format!("wire/p2/{surface}/binary").as_str()];
        gates.push(gate(
            format!("p2.{surface}.binary.latency"),
            binary.ns_per_unit,
            json.ns_per_unit * 1.25,
            "ns/request",
        ));
    }
    gates
}

fn gate(name: String, actual: f64, limit: f64, unit: &str) -> GateResult {
    GateResult {
        name,
        kind: "optimization".into(),
        passed: actual <= limit,
        actual,
        limit,
        unit: unit.into(),
    }
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

fn environment() -> Environment {
    Environment {
        os: env::consts::OS.into(),
        arch: env::consts::ARCH.into(),
        cpu_parallelism: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        rust_version: command_output("rustc", &["--version"]),
        commit: command_output("git", &["rev-parse", "HEAD"]),
        dirty: !command_output("git", &["status", "--porcelain"]).is_empty(),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .into(),
    }
}

fn command_output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into())
}
