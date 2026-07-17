use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::environment;
use crate::report::{
    BenchmarkMode, BenchmarkReport, CaseResult, Correctness, GateResult, MeasurementMode, Sampling,
    aggregate_samples,
};

#[allow(clippy::too_many_arguments)]
pub fn write(
    suite_version: &str,
    workload_version: &str,
    mode: BenchmarkMode,
    command: String,
    output: &Path,
    raw_cases: Vec<CaseResult>,
    gates: Vec<GateResult>,
    measurement_boundary: &str,
) -> Result<BenchmarkReport, String> {
    let cases = aggregate_samples(&[raw_cases], MeasurementMode::Allocation)?;
    let passed =
        gates.iter().all(|gate| gate.passed) && cases.iter().all(|case| case.correctness.passed);
    let revisions = environment::repository_revisions();
    let (environment_id, environment) = environment::capture();
    let generated_at = environment::generated_at();
    let report = BenchmarkReport {
        schema_version: "mutsuki.performance.report/v1".into(),
        suite_version: suite_version.into(),
        workload_version: workload_version.into(),
        report_id: format!(
            "{}-{}",
            suite_version.replace('/', "-"),
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
        feature_set: vec!["tracking-allocator-wire-diagnostic".into()],
        deployment: "builtin".into(),
        measurement_boundary: measurement_boundary.into(),
        sampling: Sampling {
            warmup_iterations: 0,
            samples_per_process: 1,
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
            ("command".into(), command),
            (
                "headline_eligible".into(),
                "false: tracking allocator is installed".into(),
            ),
            ("mode".into(), mode.as_str().into()),
        ]),
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        output,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(report)
}
