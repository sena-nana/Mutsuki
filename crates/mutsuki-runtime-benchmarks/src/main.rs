mod allocator;
mod batch_resource;
mod fixtures;
mod host_api;
mod longevity;
mod report;
mod scheduling;

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use allocator::TrackingAllocator;
use report::{BaselineReport, BenchmarkMode, BenchmarkReport, CaseResult, Environment, GateResult};

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateLevel {
    None,
    Smoke,
    Release,
}

struct Options {
    mode: BenchmarkMode,
    gate: GateLevel,
    output: PathBuf,
    baseline: Option<PathBuf>,
    command: String,
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
    let mut cases = Vec::new();
    cases.extend(scheduling::run(options.mode)?);
    cases.extend(longevity::run(options.mode)?);
    cases.extend(batch_resource::run(options.mode)?);
    cases.extend(host_api::run(options.mode)?);

    let baseline = options.baseline.as_deref().map(read_report).transpose()?;
    if options.gate == GateLevel::Release && baseline.is_none() {
        return Err("release gate requires --baseline <report.json>".into());
    }
    let gates = evaluate_gates(options.mode, options.gate, &cases, baseline.as_ref());
    let passed = gates.iter().all(|gate| gate.passed);
    let report = BenchmarkReport {
        schema_version: 1,
        issue: 28,
        mode: options.mode.as_str().into(),
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs(),
        command: options.command,
        environment: environment(),
        cases,
        gates,
        passed,
    };
    write_report(&options.output, &report)?;
    println!(
        "Issue #28 {} benchmark: {} cases, {} gates, result={}, report={}",
        report.mode,
        report.cases.len(),
        report.gates.len(),
        if report.passed { "PASS" } else { "FAIL" },
        options.output.display()
    );
    for gate in report.gates.iter().filter(|gate| !gate.passed) {
        eprintln!(
            "gate failed: {} actual={} {} limit={} {}",
            gate.name, gate.actual, gate.unit, gate.limit, gate.unit
        );
    }
    report
        .passed
        .then_some(())
        .ok_or_else(|| "one or more performance gates failed".into())
}

fn parse_options() -> Result<Options, String> {
    let args = env::args().collect::<Vec<_>>();
    let mode = match args.get(1).map(String::as_str) {
        Some("smoke") => BenchmarkMode::Smoke,
        Some("full") => BenchmarkMode::Full,
        _ => {
            return Err(
                "usage: mutsuki-runtime-benchmarks <smoke|full> [--gate none|smoke|release] [--output path] [--baseline path]"
                    .into(),
            );
        }
    };
    let mut gate = GateLevel::None;
    let mut output = PathBuf::from(format!(
        "target/mutsuki-benchmarks/issue28-{}.json",
        mode.as_str()
    ));
    let mut baseline = None;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
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
            other => return Err(format!("unknown benchmark argument: {other}")),
        }
        index += 1;
    }
    Ok(Options {
        mode,
        gate,
        output,
        baseline,
        command: args.join(" "),
    })
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

fn read_report(path: &Path) -> Result<BaselineReport, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read baseline {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("failed to parse baseline {}: {error}", path.display()))
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
    cases: &[CaseResult],
    baseline: Option<&BaselineReport>,
) -> Vec<GateResult> {
    if level == GateLevel::None {
        return Vec::new();
    }
    let mut gates = Vec::new();
    let scheduling_100k = cases
        .iter()
        .filter(|case| {
            case.category == "scheduling"
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
    for case in cases.iter().filter(|case| case.category == "scheduling") {
        gates.push(gate_at_most(
            format!("absolute.{}.elapsed", case.id),
            case.elapsed_ns as f64,
            30_000_000_000.0,
            "ns",
        ));
    }
    if let Some(case) = find_case(cases, "longevity/idle-tick/24h-equivalent") {
        gates.push(gate_at_most(
            "absolute.idle-tick",
            case.ns_per_unit,
            5_000_000.0,
            "ns/tick",
        ));
    }
    if let Some(case) = find_case(cases, "longevity/task-lifecycle/bounded-history") {
        gates.push(gate_at_most(
            "memory.retained-terminal-records",
            counter(case, "retained_terminal_records") as f64,
            1_024.0,
            "records",
        ));
        gates.push(gate_at_most(
            "memory.evicted-task-id-horizon",
            counter(case, "evicted_task_ids") as f64,
            2_048.0,
            "ids",
        ));
        gates.push(gate_at_most(
            "memory.completed-history-second-half-growth",
            counter(case, "second_half_retained_growth_bytes").max(0) as f64,
            8_388_608.0,
            "bytes",
        ));
    }
    if mode == BenchmarkMode::Full {
        gates.extend(full_matrix_gates(cases));
    }
    if level == GateLevel::Release
        && let Some(baseline) = baseline
    {
        gates.extend(relative_gates(cases, baseline));
    }
    gates
}

fn full_matrix_gates(cases: &[CaseResult]) -> Vec<GateResult> {
    let scheduling = cases
        .iter()
        .filter(|case| case.category == "scheduling")
        .collect::<Vec<_>>();
    let values = |key: &str| {
        scheduling
            .iter()
            .filter_map(|case| case.dimensions.get(key).cloned())
            .collect::<BTreeSet<_>>()
    };
    let expected = [
        (
            "tasks",
            BTreeSet::from(["1000".into(), "10000".into(), "100000".into()]),
        ),
        (
            "runners",
            BTreeSet::from(["1".into(), "16".into(), "128".into()]),
        ),
        (
            "ready_percent",
            BTreeSet::from(["0".into(), "1".into(), "50".into(), "100".into()]),
        ),
        (
            "batch_size",
            BTreeSet::from(["1".into(), "32".into(), "256".into()]),
        ),
        (
            "protocol_distribution",
            BTreeSet::from([
                "single_protocol".into(),
                "uniform_protocols".into(),
                "runner_hint".into(),
                "owner_continuation".into(),
            ]),
        ),
    ];
    let mut gates = expected
        .into_iter()
        .map(|(key, expected)| {
            let actual = values(key);
            GateResult {
                name: format!("matrix.{key}"),
                kind: "coverage".into(),
                passed: expected.is_subset(&actual),
                actual: actual.len() as f64,
                limit: expected.len() as f64,
                unit: "distinct-values".into(),
            }
        })
        .collect::<Vec<_>>();
    let lifecycle = find_case(cases, "longevity/task-lifecycle/bounded-history")
        .map(|case| counter(case, "lifecycle_count"))
        .unwrap_or_default();
    gates.push(gate_at_least(
        "matrix.one-million-task-lifecycles",
        lifecycle as f64,
        1_000_000.0,
        "lifecycles",
    ));
    let idle = find_case(cases, "longevity/idle-tick/24h-equivalent")
        .map(|case| case.iterations)
        .unwrap_or_default();
    gates.push(gate_at_least(
        "matrix.24h-idle-ticks",
        idle as f64,
        8_640_000.0,
        "ticks",
    ));
    let batch_cases = cases
        .iter()
        .filter(|case| case.id.starts_with("batch_resource/plan/"))
        .collect::<Vec<_>>();
    let batch_entries = batch_cases
        .iter()
        .filter_map(|case| case.dimensions.get("entries").cloned())
        .collect::<BTreeSet<_>>();
    let resource_patterns = batch_cases
        .iter()
        .filter_map(|case| case.dimensions.get("resource_pattern").cloned())
        .collect::<BTreeSet<_>>();
    gates.push(GateResult {
        name: "matrix.row-payload-entries".into(),
        kind: "coverage".into(),
        passed: BTreeSet::from(["1".into(), "32".into(), "256".into()]).is_subset(&batch_entries),
        actual: batch_entries.len() as f64,
        limit: 3.0,
        unit: "distinct-values".into(),
    });
    gates.push(GateResult {
        name: "matrix.resource-patterns".into(),
        kind: "coverage".into(),
        passed: BTreeSet::from([
            "no_resources".into(),
            "shared_read".into(),
            "write_conflict".into(),
            "strict_order".into(),
        ])
        .is_subset(&resource_patterns),
        actual: resource_patterns.len() as f64,
        limit: 4.0,
        unit: "distinct-values".into(),
    });
    let observability_states = cases
        .iter()
        .filter(|case| case.id.starts_with("longevity/observability/"))
        .filter_map(|case| case.dimensions.get("state").cloned())
        .collect::<BTreeSet<_>>();
    gates.push(GateResult {
        name: "matrix.observability-states".into(),
        kind: "coverage".into(),
        passed: BTreeSet::from(["disabled".into(), "enabled".into(), "full-capacity".into()])
            .is_subset(&observability_states),
        actual: observability_states.len() as f64,
        limit: 3.0,
        unit: "distinct-values".into(),
    });
    for required_id in [
        "longevity/deadline-cancel/cycles",
        "longevity/reload/identical-surface",
        "host/submit-batch/entries-256",
        "host/task-outcome-batch/entries-256",
        "host/events-pagination/entries-256",
        "host/traces-pagination/entries-256",
        "host/actor-command-round-trip/statistics",
    ] {
        gates.push(gate_at_least(
            format!("matrix.case.{required_id}"),
            usize::from(find_case(cases, required_id).is_some()) as f64,
            1.0,
            "case",
        ));
    }
    gates
}

fn relative_gates(cases: &[CaseResult], baseline: &BaselineReport) -> Vec<GateResult> {
    let baseline_by_id = baseline
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let mut gates = Vec::new();
    for case in cases.iter().filter(|case| {
        matches!(
            case.category.as_str(),
            "scheduling" | "batch_resource" | "host"
        )
    }) {
        let Some(previous) = baseline_by_id.get(case.id.as_str()) else {
            continue;
        };
        let time_limit = (previous.ns_per_unit * 3.0).max(previous.ns_per_unit + 50_000.0);
        gates.push(GateResult {
            name: format!("relative.{}.time", case.id),
            kind: "relative-regression".into(),
            passed: case.ns_per_unit <= time_limit,
            actual: case.ns_per_unit,
            limit: time_limit,
            unit: "ns/unit".into(),
        });
        let current_allocated = case.allocations.allocated_bytes as f64 / case.units.max(1) as f64;
        let previous_allocated =
            previous.allocations.allocated_bytes as f64 / previous.units.max(1) as f64;
        let allocation_limit = (previous_allocated * 3.0).max(previous_allocated + 4_096.0);
        gates.push(GateResult {
            name: format!("relative.{}.allocated-bytes", case.id),
            kind: "relative-regression".into(),
            passed: current_allocated <= allocation_limit,
            actual: current_allocated,
            limit: allocation_limit,
            unit: "bytes/unit".into(),
        });
    }
    gates.push(gate_at_least(
        "relative.matched-cases",
        (gates.len() / 2) as f64,
        1.0,
        "cases",
    ));
    gates
}

fn find_case<'a>(cases: &'a [CaseResult], id: &str) -> Option<&'a CaseResult> {
    cases.iter().find(|case| case.id == id)
}

fn counter(case: &CaseResult, name: &str) -> i128 {
    case.counters.get(name).copied().unwrap_or_default()
}

fn gate_at_most(
    name: impl Into<String>,
    actual: f64,
    limit: f64,
    unit: impl Into<String>,
) -> GateResult {
    GateResult {
        name: name.into(),
        kind: "absolute".into(),
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
        name: name.into(),
        kind: "coverage".into(),
        passed: actual >= limit,
        actual,
        limit,
        unit: unit.into(),
    }
}
