use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchmarkMode {
    Smoke,
    Full,
}

impl BenchmarkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Full => "full",
        }
    }

    pub fn select<T>(self, smoke: T, full: T) -> T {
        match self {
            Self::Smoke => smoke,
            Self::Full => full,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementMode {
    Time,
    Allocation,
}

impl MeasurementMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Allocation => "allocation",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AllocationMetrics {
    pub allocations: u64,
    pub deallocations: u64,
    pub allocated_bytes: u64,
    pub deallocated_bytes: u64,
    pub retained_bytes_delta: i128,
    pub peak_bytes_delta: u64,
}

/// A single in-process observation. Fixture construction happens before this
/// value is measured in each benchmark module.
#[derive(Clone, Debug)]
pub struct CaseResult {
    pub id: String,
    pub category: String,
    pub dimensions: BTreeMap<String, String>,
    pub iterations: u64,
    pub units: u64,
    pub ns_per_unit: f64,
    pub throughput_per_second: f64,
    pub allocations: AllocationMetrics,
    pub counters: BTreeMap<String, i128>,
}

impl CaseResult {
    #[allow(clippy::too_many_arguments)]
    pub fn measured(
        id: impl Into<String>,
        category: impl Into<String>,
        dimensions: BTreeMap<String, String>,
        iterations: u64,
        units: u64,
        elapsed_ns: u64,
        allocations: AllocationMetrics,
        counters: BTreeMap<String, i128>,
    ) -> Self {
        let denominator = units.max(1) as f64;
        Self {
            id: id.into(),
            category: category.into(),
            dimensions,
            iterations,
            units,
            ns_per_unit: elapsed_ns as f64 / denominator,
            throughput_per_second: denominator * 1_000_000_000.0 / elapsed_ns.max(1) as f64,
            allocations,
            counters,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepositoryRevision {
    pub revision: String,
    pub dirty: bool,
    pub remote: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseProfile {
    pub name: String,
    pub lto: String,
    pub codegen_units: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub cpu_model: String,
    pub cpu_topology: String,
    pub ram_bytes: u64,
    pub os: String,
    pub kernel: String,
    pub architecture: String,
    pub target_triple: String,
    pub toolchains: BTreeMap<String, String>,
    pub release_profile: ReleaseProfile,
    pub power_mode: String,
    pub virtualization: String,
    pub runner_configuration: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sampling {
    pub warmup_iterations: u32,
    pub samples_per_process: u32,
    pub process_runs: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Distribution {
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub mad: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
    pub sample_count: usize,
    pub samples: Vec<f64>,
}

impl Distribution {
    pub fn from_samples(samples: impl IntoIterator<Item = f64>, unit: impl Into<String>) -> Self {
        let mut samples = samples.into_iter().collect::<Vec<_>>();
        samples.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
        let median = percentile(&samples, 0.50);
        let mut deviations = samples
            .iter()
            .map(|sample| (sample - median).abs())
            .collect::<Vec<_>>();
        deviations.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
        Self {
            median,
            p95: percentile(&samples, 0.95),
            p99: percentile(&samples, 0.99),
            mad: percentile(&deviations, 0.50),
            min: samples.first().copied().unwrap_or_default(),
            max: samples.last().copied().unwrap_or_default(),
            unit: unit.into(),
            sample_count: samples.len(),
            samples,
        }
    }
}

fn percentile(samples: &[f64], quantile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() as f64 * quantile).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len() - 1);
    samples[index]
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CaseMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ns: Option<Distribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throughput_per_second: Option<Distribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_time_ns: Option<Distribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocations: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocated_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_rss_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_switches: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Correctness {
    pub passed: bool,
    pub counters: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseReport {
    pub case_id: String,
    pub measurement_mode: String,
    pub dimensions: BTreeMap<String, String>,
    pub metrics: CaseMetrics,
    pub correctness: Correctness,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub stage_breakdown: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateResult {
    pub gate_id: String,
    pub kind: String,
    pub passed: bool,
    pub actual: f64,
    pub limit: f64,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub schema_version: String,
    pub suite_version: String,
    pub workload_version: String,
    pub report_id: String,
    pub generated_at: String,
    pub revision_lock_hash: String,
    pub repository_revisions: BTreeMap<String, RepositoryRevision>,
    pub environment_id: String,
    pub environment: Environment,
    pub feature_set: Vec<String>,
    pub deployment: String,
    pub measurement_boundary: String,
    pub sampling: Sampling,
    pub cases: Vec<CaseReport>,
    pub correctness: Correctness,
    pub gates: Vec<GateResult>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BaselineReport {
    pub environment_id: String,
    pub measurement_boundary: String,
    pub cases: Vec<CaseReport>,
}

pub fn aggregate_samples(
    rounds: &[Vec<CaseResult>],
    measurement_mode: MeasurementMode,
) -> Result<Vec<CaseReport>, String> {
    let first = rounds
        .first()
        .ok_or_else(|| "benchmark sample set cannot be empty".to_string())?;
    let mut reports = Vec::with_capacity(first.len());
    for (index, template) in first.iter().enumerate() {
        let observations = rounds
            .iter()
            .map(|round| {
                let observed = round
                    .get(index)
                    .ok_or_else(|| "benchmark case count changed between samples".to_string())?;
                if observed.id != template.id || observed.dimensions != template.dimensions {
                    return Err(format!(
                        "benchmark case order changed between samples: {} vs {}",
                        template.id, observed.id
                    ));
                }
                Ok(observed)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut dimensions = template.dimensions.clone();
        dimensions.insert("legacy_case_id".into(), template.id.clone());
        dimensions.insert("category".into(), template.category.clone());
        dimensions.insert("iterations".into(), template.iterations.to_string());
        dimensions.insert("units".into(), template.units.to_string());
        let metrics = match measurement_mode {
            MeasurementMode::Time => CaseMetrics {
                latency_ns: Some(Distribution::from_samples(
                    observations.iter().map(|case| case.ns_per_unit),
                    "ns/unit",
                )),
                throughput_per_second: Some(Distribution::from_samples(
                    observations.iter().map(|case| case.throughput_per_second),
                    "units/s",
                )),
                ..CaseMetrics::default()
            },
            MeasurementMode::Allocation => {
                let units = template.units.max(1) as f64;
                CaseMetrics {
                    allocations: Some(
                        observations
                            .iter()
                            .map(|case| case.allocations.allocations as f64 / units)
                            .sum::<f64>()
                            / observations.len() as f64,
                    ),
                    allocated_bytes: Some(
                        observations
                            .iter()
                            .map(|case| case.allocations.allocated_bytes as f64 / units)
                            .sum::<f64>()
                            / observations.len() as f64,
                    ),
                    retained_rss_bytes: Some(
                        observations
                            .iter()
                            .map(|case| case.allocations.retained_bytes_delta as f64)
                            .sum::<f64>()
                            / observations.len() as f64,
                    ),
                    ..CaseMetrics::default()
                }
            }
        };
        let counters = template
            .counters
            .iter()
            .map(|(name, value)| {
                let value = i64::try_from(*value)
                    .map_err(|_| format!("counter {name} does not fit in i64"))?;
                Ok((name.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        reports.push(CaseReport {
            case_id: standard_case_id(template),
            measurement_mode: measurement_mode.as_str().into(),
            dimensions,
            metrics,
            correctness: Correctness {
                passed: true,
                counters,
                output_hash: None,
            },
            stage_breakdown: BTreeMap::new(),
        });
    }
    Ok(reports)
}

fn standard_case_id(case: &CaseResult) -> String {
    let id = case.id.as_str();
    if id == "longevity/idle-tick/24h-equivalent" {
        return "core.idle-runtime".into();
    }
    if id.contains("task-lifecycle") {
        return "core.task-lifecycle".into();
    }
    if id.contains("deadline-cancel") {
        return "core.deadline-cancel".into();
    }
    if id.contains("reload") {
        return "core.reload".into();
    }
    if id.starts_with("scheduling/") {
        if case
            .dimensions
            .get("protocol_distribution")
            .is_some_and(|value| value == "owner_continuation")
        {
            return "core.wait-wake".into();
        }
        return if case
            .dimensions
            .get("ready_percent")
            .is_some_and(|value| value == "100")
        {
            "core.schedule.full-ready"
        } else {
            "core.schedule.sparse-ready"
        }
        .into();
    }
    if id.starts_with("batch_resource/plan/") {
        let suffix = match case.dimensions.get("resource_pattern").map(String::as_str) {
            Some("shared_read") => "shared-read",
            Some("write_conflict") => "write-conflict",
            Some("strict_order") => "strict-order",
            _ => "none",
        };
        return format!("core.resource-plan.{suffix}");
    }
    if id.starts_with("batch_resource/completion/") {
        return "core.completion-route".into();
    }
    if id.starts_with("host/submit-batch/") {
        return "core.host.submit-batch".into();
    }
    if id.starts_with("host/task-outcome") {
        return "core.host.task-outcome".into();
    }
    if id.starts_with("host/events-")
        || id.starts_with("host/traces-")
        || id.starts_with("longevity/observability/")
    {
        return "core.host.observability-page".into();
    }
    if id.starts_with("host/actor-command") {
        return "core.host.actor-command".into();
    }
    if id.starts_with("wire/") {
        return format!(
            "core.wire.{}",
            id.trim_start_matches("wire/")
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() {
                        character.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .trim_matches('-')
        );
    }
    format!(
        "core.legacy.{}",
        id.chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_reports_ordered_percentiles_and_mad() {
        let distribution = Distribution::from_samples([1.0, 2.0, 3.0, 100.0], "ns");
        assert_eq!(distribution.median, 2.0);
        assert_eq!(distribution.p95, 100.0);
        assert_eq!(distribution.p99, 100.0);
        assert_eq!(distribution.mad, 1.0);
        assert!(distribution.min <= distribution.median);
        assert!(distribution.median <= distribution.p95);
        assert!(distribution.p95 <= distribution.p99);
        assert!(distribution.p99 <= distribution.max);
    }
}
