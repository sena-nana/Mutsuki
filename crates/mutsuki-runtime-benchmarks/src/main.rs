mod allocator;
mod batch_resource;
mod environment;
mod fixtures;
mod host_api;
mod longevity;
mod report;
mod scheduling;
mod system_metrics;

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use allocator::TrackingAllocator;
use report::{
    BaselineReport, BenchmarkMode, BenchmarkReport, CaseReport, Correctness, GateResult,
    MeasurementMode, Sampling, aggregate_samples,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(feature = "allocation-tracking")]
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

#[cfg(not(feature = "allocation-tracking"))]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateLevel {
    None,
    Smoke,
    Release,
}

struct Options {
    mode: BenchmarkMode,
    measurement_mode: MeasurementMode,
    gate: GateLevel,
    output: PathBuf,
    baseline: Option<PathBuf>,
    baseline_approval: Option<PathBuf>,
    warmup_iterations: u32,
    samples: u32,
    command: String,
}

#[derive(Deserialize)]
struct BaselineApproval {
    schema_version: String,
    report_sha256: String,
    revision_lock_hash: String,
    environment_id: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("benchmark failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    validate_lane_build(options.measurement_mode)?;
    for _ in 0..options.warmup_iterations {
        let _ = run_cases(options.mode)?;
    }
    let rounds = (0..options.samples)
        .map(|_| run_cases(options.mode))
        .collect::<Result<Vec<_>, _>>()?;
    let mut cases = aggregate_samples(&rounds, options.measurement_mode)?;
    if options.measurement_mode == MeasurementMode::Time {
        cases.push(system_metrics::process_case());
    }

    let baseline = options.baseline.as_deref().map(read_baseline).transpose()?;
    if options.gate == GateLevel::Release {
        let baseline_path = options
            .baseline
            .as_deref()
            .ok_or_else(|| "release gate requires --baseline <report.json>".to_string())?;
        let approval_path = options.baseline_approval.as_deref().ok_or_else(|| {
            "release gate requires --baseline-approval <approval.json>".to_string()
        })?;
        verify_baseline_approval(baseline_path, approval_path)?;
    }

    let (environment_id, environment) = environment::capture();
    if let Some(baseline) = &baseline
        && options.gate == GateLevel::Release
        && (baseline.environment_id != environment_id
            || baseline.measurement_boundary != measurement_boundary(options.measurement_mode))
    {
        return Err("release baseline environment or measurement boundary does not match".into());
    }
    let gates = evaluate_gates(options.mode, options.gate, &cases, baseline.as_ref());
    let passed =
        gates.iter().all(|gate| gate.passed) && cases.iter().all(|case| case.correctness.passed);
    let revisions = environment::repository_revisions();
    let generated_at = environment::generated_at();
    let report = BenchmarkReport {
        schema_version: "mutsuki.performance.report/v1".into(),
        suite_version: "mutsuki-core/v2".into(),
        workload_version: "mutsuki-core-kernel/v1".into(),
        report_id: format!(
            "core-{}-{}-{}",
            options.mode.as_str(),
            options.measurement_mode.as_str(),
            generated_at
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        ),
        generated_at,
        revision_lock_hash: environment::revision_lock_hash(&revisions),
        repository_revisions: revisions,
        environment_id,
        environment,
        feature_set: if cfg!(feature = "allocation-tracking") {
            vec!["allocation-tracking".into()]
        } else {
            Vec::new()
        },
        deployment: "builtin".into(),
        measurement_boundary: measurement_boundary(options.measurement_mode).into(),
        sampling: Sampling {
            warmup_iterations: options.warmup_iterations,
            samples_per_process: options.samples,
            process_runs: 1,
        },
        cases,
        correctness: Correctness {
            passed,
            counters: BTreeMap::from([(
                "failed_gates".into(),
                gates.iter().filter(|gate| !gate.passed).count() as i64,
            )]),
            output_hash: None,
        },
        gates,
        metadata: BTreeMap::from([
            ("command".into(), options.command),
            (
                "fixture_window".into(),
                "fixture construction is outside each case measurement window".into(),
            ),
        ]),
    };
    write_report(&options.output, &report)?;
    println!(
        "Core {} {} benchmark: {} cases, {} gates, result={}, report={}",
        options.mode.as_str(),
        options.measurement_mode.as_str(),
        report.cases.len(),
        report.gates.len(),
        if report.correctness.passed {
            "PASS"
        } else {
            "FAIL"
        },
        options.output.display()
    );
    for gate in report.gates.iter().filter(|gate| !gate.passed) {
        eprintln!(
            "gate failed: {} actual={} {} limit={} {}",
            gate.gate_id, gate.actual, gate.unit, gate.limit, gate.unit
        );
    }
    report
        .correctness
        .passed
        .then_some(())
        .ok_or_else(|| "one or more performance gates failed".into())
}

fn run_cases(mode: BenchmarkMode) -> Result<Vec<report::CaseResult>, String> {
    let mut cases = Vec::new();
    cases.extend(scheduling::run(mode)?);
    cases.extend(longevity::run(mode)?);
    cases.extend(batch_resource::run(mode)?);
    cases.extend(host_api::run(mode)?);
    Ok(cases)
}

fn measurement_boundary(mode: MeasurementMode) -> &'static str {
    match mode {
        MeasurementMode::Time => {
            "Core runtime kernel and Host-facing component time; system allocator; no ABI/process/network"
        }
        MeasurementMode::Allocation => {
            "Core runtime kernel and Host-facing component allocation instrumentation; timing is non-headline"
        }
    }
}

fn validate_lane_build(mode: MeasurementMode) -> Result<(), String> {
    match (mode, cfg!(feature = "allocation-tracking")) {
        (MeasurementMode::Time, false) | (MeasurementMode::Allocation, true) => Ok(()),
        (MeasurementMode::Time, true) => Err(
            "time lane must be built without the allocation-tracking feature so the system allocator remains active"
                .into(),
        ),
        (MeasurementMode::Allocation, false) => Err(
            "allocation lane requires --features allocation-tracking and a separate process".into(),
        ),
    }
}

fn parse_options() -> Result<Options, String> {
    let args = env::args().collect::<Vec<_>>();
    let mode = match args.get(1).map(String::as_str) {
        Some("smoke") => BenchmarkMode::Smoke,
        Some("full") => BenchmarkMode::Full,
        _ => {
            return Err(
                "usage: mutsuki-runtime-benchmarks <smoke|full> [--lane time|allocation] [--warmup N] [--samples N] [--gate none|smoke|release] [--output path] [--baseline path --baseline-approval path]"
                    .into(),
            );
        }
    };
    let mut measurement_mode = MeasurementMode::Time;
    let mut gate = GateLevel::None;
    let mut output = PathBuf::from(format!(
        "target/mutsuki-benchmarks/core-{}-time.json",
        mode.as_str()
    ));
    let mut baseline = None;
    let mut baseline_approval = None;
    let mut warmup_iterations = mode.select(0, 1);
    let mut samples = mode.select(1, 5);
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--lane" => {
                index += 1;
                measurement_mode = match args.get(index).map(String::as_str) {
                    Some("time") => MeasurementMode::Time,
                    Some("allocation") => MeasurementMode::Allocation,
                    _ => return Err("--lane expects time or allocation".into()),
                };
            }
            "--warmup" => {
                index += 1;
                warmup_iterations = parse_count(args.get(index), "--warmup", true)?;
            }
            "--samples" => {
                index += 1;
                samples = parse_count(args.get(index), "--samples", false)?;
            }
            "--gate" => {
                index += 1;
                gate = match args.get(index).map(String::as_str) {
                    Some("none") => GateLevel::None,
                    Some("smoke") => GateLevel::Smoke,
                    Some("release") => GateLevel::Release,
                    _ => return Err("--gate expects none, smoke, or release".into()),
                };
            }
            "--output" => {
                index += 1;
                output = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--output expects a path".to_string())?,
                );
            }
            "--baseline" => {
                index += 1;
                baseline = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--baseline expects a path".to_string())?,
                ));
            }
            "--baseline-approval" => {
                index += 1;
                baseline_approval =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        "--baseline-approval expects a path".to_string()
                    })?));
            }
            other => return Err(format!("unknown benchmark argument: {other}")),
        }
        index += 1;
    }
    if measurement_mode == MeasurementMode::Allocation
        && output == format!("target/mutsuki-benchmarks/core-{}-time.json", mode.as_str())
    {
        output = PathBuf::from(format!(
            "target/mutsuki-benchmarks/core-{}-allocation.json",
            mode.as_str()
        ));
    }
    Ok(Options {
        mode,
        measurement_mode,
        gate,
        output,
        baseline,
        baseline_approval,
        warmup_iterations,
        samples,
        command: sanitized_command(&args),
    })
}

fn sanitized_command(args: &[String]) -> String {
    let mut command = args.to_vec();
    let mut index = 0;
    while index + 1 < command.len() {
        let replacement = match command[index].as_str() {
            "--output" => Some("$OUTPUT"),
            "--baseline" => Some("$BASELINE"),
            "--baseline-approval" => Some("$BASELINE_APPROVAL"),
            _ => None,
        };
        if let Some(replacement) = replacement {
            command[index + 1] = replacement.into();
            index += 1;
        }
        index += 1;
    }
    command.join(" ")
}

fn parse_count(value: Option<&String>, option: &str, allow_zero: bool) -> Result<u32, String> {
    let value = value
        .ok_or_else(|| format!("{option} expects an integer"))?
        .parse::<u32>()
        .map_err(|_| format!("{option} expects an integer"))?;
    if !allow_zero && value == 0 {
        return Err(format!("{option} must be at least 1"));
    }
    Ok(value)
}

fn read_baseline(path: &Path) -> Result<BaselineReport, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read baseline {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("failed to parse baseline {}: {error}", path.display()))
}

fn verify_baseline_approval(report_path: &Path, approval_path: &Path) -> Result<(), String> {
    let report = fs::read(report_path)
        .map_err(|error| format!("failed to read baseline {}: {error}", report_path.display()))?;
    let approval: BaselineApproval =
        serde_json::from_slice(&fs::read(approval_path).map_err(|error| {
            format!(
                "failed to read baseline approval {}: {error}",
                approval_path.display()
            )
        })?)
        .map_err(|error| format!("failed to parse baseline approval: {error}"))?;
    if approval.schema_version != "mutsuki.performance.baseline-approval/v1" {
        return Err("baseline approval uses an unsupported schema version".into());
    }
    let digest = format!("{:x}", Sha256::digest(&report));
    if digest != approval.report_sha256 {
        return Err("baseline approval does not match the report SHA-256".into());
    }
    let baseline: BenchmarkReport = serde_json::from_slice(&report)
        .map_err(|error| format!("failed to parse approved baseline: {error}"))?;
    if baseline.revision_lock_hash != approval.revision_lock_hash
        || baseline.environment_id != approval.environment_id
    {
        return Err("baseline approval metadata does not match the report".into());
    }
    if baseline
        .repository_revisions
        .values()
        .any(|revision| revision.dirty)
    {
        return Err("a dirty repository report cannot be an approved baseline".into());
    }
    Ok(())
}

fn write_report(path: &Path, report: &BenchmarkReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, format!("{json}\n"))
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn evaluate_gates(
    mode: BenchmarkMode,
    level: GateLevel,
    cases: &[CaseReport],
    baseline: Option<&BaselineReport>,
) -> Vec<GateResult> {
    if level == GateLevel::None {
        return Vec::new();
    }
    let mut gates = Vec::new();
    let scheduling_100k = cases
        .iter()
        .filter(|case| {
            case.case_id.starts_with("core.schedule.")
                && case
                    .dimensions
                    .get("tasks")
                    .is_some_and(|value| value == "100000")
        })
        .count();
    gates.push(gate_at_least(
        "matrix.100k-task-scheduling",
        scheduling_100k as f64,
        1.0,
        "cases",
    ));
    for case in cases.iter().filter(|case| case.measurement_mode == "time") {
        if let Some(latency) = &case.metrics.latency_ns {
            gates.push(gate_at_most(
                format!("absolute.{}.p99", case.case_id),
                latency.p99,
                30_000_000_000.0,
                "ns/unit",
            ));
        }
    }
    if mode == BenchmarkMode::Full {
        gates.extend(full_matrix_gates(cases));
    }
    if level == GateLevel::Release
        && let Some(baseline) = baseline
    {
        gates.extend(relative_gates(cases, baseline));
    }
    gates.extend(zero_tolerance_gates(cases));
    gates
}

fn full_matrix_gates(cases: &[CaseReport]) -> Vec<GateResult> {
    let values = |key: &str| {
        cases
            .iter()
            .filter_map(|case| case.dimensions.get(key).cloned())
            .collect::<BTreeSet<_>>()
    };
    let expected: [(&str, &[&str]); 5] = [
        ("tasks", &["1000", "10000", "100000"]),
        ("runners", &["1", "16", "128"]),
        ("ready_percent", &["0", "1", "50", "100"]),
        ("batch_size", &["1", "32", "256"]),
        (
            "protocol_distribution",
            &[
                "single_protocol",
                "uniform_protocols",
                "runner_hint",
                "owner_continuation",
            ],
        ),
    ];
    let mut gates = expected
        .into_iter()
        .map(|(key, expected)| coverage_gate(format!("matrix.{key}"), &values(key), expected))
        .collect::<Vec<_>>();
    for required in [
        "core.idle-runtime",
        "core.schedule.sparse-ready",
        "core.schedule.full-ready",
        "core.task-lifecycle",
        "core.wait-wake",
        "core.deadline-cancel",
        "core.reload",
        "core.resource-plan.none",
        "core.resource-plan.shared-read",
        "core.resource-plan.write-conflict",
        "core.resource-plan.strict-order",
        "core.completion-route",
        "core.host.submit-batch",
        "core.host.task-outcome",
        "core.host.observability-page",
        "core.host.actor-command",
    ] {
        gates.push(gate_at_least(
            format!("matrix.case.{required}"),
            usize::from(cases.iter().any(|case| case.case_id == required)) as f64,
            1.0,
            "case",
        ));
    }
    let lifecycle = cases
        .iter()
        .find(|case| case.case_id == "core.task-lifecycle")
        .and_then(|case| case.correctness.counters.get("lifecycle_count"))
        .copied()
        .unwrap_or_default();
    gates.push(gate_at_least(
        "matrix.one-million-task-lifecycles",
        lifecycle as f64,
        1_000_000.0,
        "lifecycles",
    ));
    gates
}

fn coverage_gate(
    name: impl Into<String>,
    actual: &BTreeSet<String>,
    expected: &[&str],
) -> GateResult {
    GateResult {
        gate_id: name.into(),
        kind: "coverage".into(),
        passed: expected.iter().all(|value| actual.contains(*value)),
        actual: actual.len() as f64,
        limit: expected.len() as f64,
        unit: "distinct-values".into(),
    }
}

fn relative_gates(cases: &[CaseReport], baseline: &BaselineReport) -> Vec<GateResult> {
    let baseline_by_key = baseline
        .cases
        .iter()
        .map(|case| (case_key(case), case))
        .collect::<BTreeMap<_, _>>();
    let mut gates = Vec::new();
    for case in cases {
        let Some(previous) = baseline_by_key.get(&case_key(case)) else {
            continue;
        };
        if let (Some(current), Some(old)) = (&case.metrics.latency_ns, &previous.metrics.latency_ns)
        {
            let median_limit = old.median + (old.median * 0.10).max(old.mad * 3.0);
            gates.push(gate_at_most(
                format!("relative.{}.median", case.case_id),
                current.median,
                median_limit,
                "ns/unit",
            ));
            gates.push(gate_at_most(
                format!("relative.{}.p99", case.case_id),
                current.p99,
                old.p99 * 1.20,
                "ns/unit",
            ));
        }
        if let (Some(current), Some(old)) = (
            &case.metrics.throughput_per_second,
            &previous.metrics.throughput_per_second,
        ) {
            gates.push(gate_at_least(
                format!("relative.{}.throughput", case.case_id),
                current.median,
                old.median * 0.90,
                "units/s",
            ));
        }
        if let (Some(current), Some(old)) = (
            case.metrics.allocated_bytes,
            previous.metrics.allocated_bytes,
        ) {
            gates.push(gate_at_most(
                format!("relative.{}.allocated-bytes", case.case_id),
                current,
                old + (old * 0.10).max(64.0),
                "bytes/unit",
            ));
        }
        if let (Some(current), Some(old)) =
            (case.metrics.peak_rss_bytes, previous.metrics.peak_rss_bytes)
        {
            gates.push(gate_at_most(
                format!("relative.{}.peak-rss", case.case_id),
                current,
                old * 1.10,
                "bytes",
            ));
        }
    }
    gates.push(gate_at_least(
        "relative.matched-cases",
        gates.len() as f64,
        1.0,
        "comparisons",
    ));
    gates
}

fn case_key(case: &CaseReport) -> String {
    let mut dimensions = case.dimensions.clone();
    dimensions.remove("iterations");
    dimensions.remove("units");
    format!(
        "{}|{}|{}",
        case.case_id,
        case.measurement_mode,
        serde_json::to_string(&dimensions).expect("dimensions must serialize")
    )
}

fn zero_tolerance_gates(cases: &[CaseReport]) -> Vec<GateResult> {
    const COUNTERS: [&str; 5] = [
        "duplicate_committed_results",
        "stale_results_accepted",
        "unsafe_retries",
        "unsafe_remote_placements",
        "duplicate_execution",
    ];
    let mut gates = Vec::new();
    for case in cases {
        for counter in COUNTERS {
            if let Some(value) = case.correctness.counters.get(counter) {
                gates.push(gate_at_most(
                    format!("correctness.{}.{}", case.case_id, counter),
                    *value as f64,
                    0.0,
                    "events",
                ));
            }
        }
        if let Some(slope) = case
            .correctness
            .counters
            .get("retained_growth_slope_bytes_per_sample")
        {
            gates.push(gate_at_most(
                format!("memory.{}.retained-growth-slope", case.case_id),
                *slope as f64,
                0.0,
                "bytes/sample",
            ));
        }
    }
    gates
}

fn gate_at_most(
    name: impl Into<String>,
    actual: f64,
    limit: f64,
    unit: impl Into<String>,
) -> GateResult {
    GateResult {
        gate_id: name.into(),
        kind: "maximum".into(),
        passed: actual <= limit,
        actual,
        limit,
        unit: unit.into(),
    }
}

fn gate_at_least(
    name: impl Into<String>,
    actual: f64,
    limit: f64,
    unit: impl Into<String>,
) -> GateResult {
    GateResult {
        gate_id: name.into(),
        kind: "minimum".into(),
        passed: actual >= limit,
        actual,
        limit,
        unit: unit.into(),
    }
}
