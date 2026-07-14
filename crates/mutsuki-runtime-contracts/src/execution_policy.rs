use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ERR_EXECUTION_NO_VARIANT, ProtocolId, RuntimeError, ScalarValue};

pub const EXECUTION_PROFILE_WINDOW_CAPACITY: usize = 32;
pub const EXECUTION_PROFILE_HISTOGRAM_BUCKETS: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    HardRealtime,
    SoftRealtime,
    Interactive,
    #[default]
    Batch,
    Background,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Criticality {
    SafetyCritical,
    MissionCritical,
    UserVisible,
    #[default]
    Normal,
    Deferrable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    Fail,
    Wait,
    #[default]
    Fallback,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoPlacementPolicy {
    Fail,
    Wait,
    #[default]
    UseFallback,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicy {
    #[default]
    Disabled,
    FreshOnly,
    AllowStale {
        max_age_ms: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialResultPolicy {
    #[default]
    Reject,
    Allow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityPolicy {
    pub requested_level: u8,
    pub minimum_level: u8,
}

impl Default for QualityPolicy {
    fn default() -> Self {
        Self {
            requested_level: u8::MAX,
            minimum_level: u8::MAX,
        }
    }
}

impl QualityPolicy {
    pub fn permits_degradation(&self) -> bool {
        self.minimum_level < self.requested_level
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadlinePolicy {
    pub deadline_after_ms: Option<u64>,
    pub max_queue_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSchedulingHint {
    pub criticality: Criticality,
    pub base_priority: i64,
    pub preemptible: bool,
    pub pausable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    pub latency_class: LatencyClass,
    pub deadline: DeadlinePolicy,
    pub failure_mode: FailureMode,
    pub quality: QualityPolicy,
    pub cache: CachePolicy,
    pub partial_results: PartialResultPolicy,
    pub no_placement: NoPlacementPolicy,
    pub scheduling: LocalSchedulingHint,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    pub architectures: BTreeSet<String>,
    pub instruction_sets: BTreeSet<String>,
    pub compute_backends: BTreeSet<String>,
    pub precisions: BTreeSet<String>,
    pub memory_classes: BTreeSet<String>,
    pub available_memory_bytes: Option<u64>,
    pub runner_versions: BTreeMap<String, String>,
    pub plugin_versions: BTreeMap<String, String>,
    pub custom: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementSet {
    pub architectures: BTreeSet<String>,
    pub instruction_sets: BTreeSet<String>,
    pub compute_backends: BTreeSet<String>,
    pub precisions: BTreeSet<String>,
    pub memory_classes: BTreeSet<String>,
    pub minimum_memory_bytes: Option<u64>,
    pub runner_versions: BTreeMap<String, String>,
    pub plugin_versions: BTreeMap<String, String>,
    pub custom: BTreeSet<String>,
}

impl RequirementSet {
    pub fn is_satisfied_by(&self, capabilities: &CapabilitySet) -> bool {
        self.architectures.is_subset(&capabilities.architectures)
            && self
                .instruction_sets
                .is_subset(&capabilities.instruction_sets)
            && self
                .compute_backends
                .is_subset(&capabilities.compute_backends)
            && self.precisions.is_subset(&capabilities.precisions)
            && self.memory_classes.is_subset(&capabilities.memory_classes)
            && self.minimum_memory_bytes.is_none_or(|required| {
                capabilities
                    .available_memory_bytes
                    .is_some_and(|available| available >= required)
            })
            && versions_match(&self.runner_versions, &capabilities.runner_versions)
            && versions_match(&self.plugin_versions, &capabilities.plugin_versions)
            && self.custom.is_subset(&capabilities.custom)
    }
}

fn versions_match(
    required: &BTreeMap<String, String>,
    available: &BTreeMap<String, String>,
) -> bool {
    required
        .iter()
        .all(|(id, version)| available.get(id) == Some(version))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionVariant {
    pub variant_id: String,
    pub task_type: ProtocolId,
    pub runner_id: String,
    pub plugin_id: String,
    pub implementation_version: String,
    pub requirements: RequirementSet,
    pub quality_level: u8,
    pub preference: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionVariantCatalog {
    pub variants: Vec<ExecutionVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantSelection {
    pub variant: ExecutionVariant,
    pub outcome: ExecutionOutcomeMetadata,
}

impl ExecutionVariantCatalog {
    #[allow(clippy::result_large_err)]
    pub fn select_local(
        &self,
        task_type: &str,
        capabilities: &CapabilitySet,
        policy: &ExecutionPolicy,
    ) -> Result<VariantSelection, RuntimeError> {
        let mut all: Vec<_> = self
            .variants
            .iter()
            .filter(|variant| variant.task_type == task_type)
            .collect();
        all.sort_by(variant_order);
        let preferred_variant_id = all.first().map(|variant| variant.variant_id.clone());
        let mut compatible: Vec<_> = all
            .iter()
            .copied()
            .filter(|variant| variant.requirements.is_satisfied_by(capabilities))
            .collect();
        compatible.sort_by(variant_order);

        let selected = compatible
            .iter()
            .copied()
            .find(|variant| variant.quality_level >= policy.quality.requested_level)
            .or_else(|| {
                (policy.no_placement == NoPlacementPolicy::UseFallback
                    && policy.quality.permits_degradation())
                .then(|| {
                    compatible
                        .iter()
                        .copied()
                        .find(|variant| variant.quality_level >= policy.quality.minimum_level)
                })
                .flatten()
            });

        let Some(selected) = selected else {
            let mut error = RuntimeError::new(
                ERR_EXECUTION_NO_VARIANT,
                "runtime.execution_policy",
                format!("execution.select.{task_type}"),
            );
            error.recovery = match policy.no_placement {
                NoPlacementPolicy::Wait => Some("wait_for_local_capability".into()),
                NoPlacementPolicy::UseFallback => Some("provide_explicit_fallback".into()),
                NoPlacementPolicy::Fail => None,
            };
            error.evidence.insert(
                "compatible_variants".into(),
                ScalarValue::Int(compatible.len() as i64),
            );
            return Err(error);
        };

        let fallback_used = preferred_variant_id.as_deref() != Some(selected.variant_id.as_str());
        let degraded = selected.quality_level < policy.quality.requested_level;
        let mut degradation_reasons = Vec::new();
        if fallback_used {
            degradation_reasons.push("preferred_variant_unavailable".into());
        }
        if degraded {
            degradation_reasons.push("requested_quality_unavailable".into());
        }
        Ok(VariantSelection {
            variant: selected.clone(),
            outcome: ExecutionOutcomeMetadata {
                requested_variant_id: preferred_variant_id,
                executed_variant_id: Some(selected.variant_id.clone()),
                quality_level: selected.quality_level,
                partial: false,
                stale: false,
                fallback_used,
                degradation_reasons,
            },
        })
    }
}

fn variant_order(left: &&ExecutionVariant, right: &&ExecutionVariant) -> std::cmp::Ordering {
    left.preference
        .cmp(&right.preference)
        .then_with(|| right.quality_level.cmp(&left.quality_level))
        .then_with(|| left.variant_id.cmp(&right.variant_id))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionOutcomeMetadata {
    pub requested_variant_id: Option<String>,
    pub executed_variant_id: Option<String>,
    pub quality_level: u8,
    pub partial: bool,
    pub stale: bool,
    pub fallback_used: bool,
    pub degradation_reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixedEwma {
    pub alpha: f64,
    pub value: f64,
    pub initialized: bool,
}

impl Default for FixedEwma {
    fn default() -> Self {
        Self {
            alpha: 0.2,
            value: 0.0,
            initialized: false,
        }
    }
}

impl FixedEwma {
    pub fn record(&mut self, sample: f64) {
        if self.initialized {
            self.value = self.alpha * sample + (1.0 - self.alpha) * self.value;
        } else {
            self.value = sample;
            self.initialized = true;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedHistogram {
    pub upper_bounds_ms: [u64; EXECUTION_PROFILE_HISTOGRAM_BUCKETS],
    pub counts: [u64; EXECUTION_PROFILE_HISTOGRAM_BUCKETS + 1],
    pub total: u64,
}

impl Default for FixedHistogram {
    fn default() -> Self {
        Self {
            upper_bounds_ms: [
                1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1_000, 2_000, 4_000, 8_000, 16_000, 32_000,
            ],
            counts: [0; EXECUTION_PROFILE_HISTOGRAM_BUCKETS + 1],
            total: 0,
        }
    }
}

impl FixedHistogram {
    pub fn record(&mut self, sample_ms: u64) {
        let index = self
            .upper_bounds_ms
            .iter()
            .position(|bound| sample_ms <= *bound)
            .unwrap_or(EXECUTION_PROFILE_HISTOGRAM_BUCKETS);
        self.counts[index] = self.counts[index].saturating_add(1);
        self.total = self.total.saturating_add(1);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedSampleWindow {
    samples: [u64; EXECUTION_PROFILE_WINDOW_CAPACITY],
    next: u8,
    len: u8,
}

impl Default for FixedSampleWindow {
    fn default() -> Self {
        Self {
            samples: [0; EXECUTION_PROFILE_WINDOW_CAPACITY],
            next: 0,
            len: 0,
        }
    }
}

impl FixedSampleWindow {
    pub fn record(&mut self, sample: u64) {
        self.samples[self.next as usize] = sample;
        self.next = ((self.next as usize + 1) % EXECUTION_PROFILE_WINDOW_CAPACITY) as u8;
        self.len = self
            .len
            .saturating_add(1)
            .min(EXECUTION_PROFILE_WINDOW_CAPACITY as u8);
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn quantile(&self, percentile: u8) -> Option<u64> {
        if self.is_empty() {
            return None;
        }
        let mut sorted = [0u64; EXECUTION_PROFILE_WINDOW_CAPACITY];
        let len = self.len();
        sorted[..len].copy_from_slice(&self.samples[..len]);
        sorted[..len].sort_unstable();
        let rank = (percentile.min(100) as usize * (len - 1)).div_ceil(100);
        Some(sorted[rank])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionSample {
    pub latency_ms: u64,
    pub throughput_per_second: f64,
    pub peak_memory_bytes: u64,
    pub succeeded: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionProfileAccumulator {
    pub task_type: ProtocolId,
    pub variant_id: String,
    pub input_bucket: String,
    pub latency_window: FixedSampleWindow,
    pub latency_histogram: FixedHistogram,
    pub throughput: FixedEwma,
    pub peak_memory_bytes: u64,
    pub failures: u64,
    pub sample_count: u64,
}

impl ExecutionProfileAccumulator {
    pub fn new(
        task_type: impl Into<String>,
        variant_id: impl Into<String>,
        input_bucket: impl Into<String>,
    ) -> Self {
        Self {
            task_type: task_type.into(),
            variant_id: variant_id.into(),
            input_bucket: input_bucket.into(),
            latency_window: FixedSampleWindow::default(),
            latency_histogram: FixedHistogram::default(),
            throughput: FixedEwma::default(),
            peak_memory_bytes: 0,
            failures: 0,
            sample_count: 0,
        }
    }

    pub fn record(&mut self, sample: ExecutionSample) {
        self.latency_window.record(sample.latency_ms);
        self.latency_histogram.record(sample.latency_ms);
        self.throughput.record(sample.throughput_per_second);
        self.peak_memory_bytes = self.peak_memory_bytes.max(sample.peak_memory_bytes);
        self.sample_count = self.sample_count.saturating_add(1);
        if !sample.succeeded {
            self.failures = self.failures.saturating_add(1);
        }
    }

    pub fn snapshot(&self) -> ExecutionProfile {
        ExecutionProfile {
            task_type: self.task_type.clone(),
            variant_id: self.variant_id.clone(),
            input_bucket: self.input_bucket.clone(),
            p50_ms: self.latency_window.quantile(50).unwrap_or(0),
            p95_ms: self.latency_window.quantile(95).unwrap_or(0),
            p99_ms: self.latency_window.quantile(99).unwrap_or(0),
            throughput_per_second: self.throughput.value,
            peak_memory_bytes: self.peak_memory_bytes,
            failure_rate: if self.sample_count == 0 {
                0.0
            } else {
                self.failures as f64 / self.sample_count as f64
            },
            sample_count: self.sample_count,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionProfile {
    pub task_type: ProtocolId,
    pub variant_id: String,
    pub input_bucket: String,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
    pub throughput_per_second: f64,
    pub peak_memory_bytes: u64,
    pub failure_rate: f64,
    pub sample_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilingBudget {
    pub max_samples_per_interval: u64,
    pub max_cpu_time_ms_per_interval: u64,
    pub max_memory_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PressureLevel {
    Idle,
    #[default]
    Normal,
    Elevated,
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn variant(
        id: &str,
        backend: &str,
        precision: &str,
        quality: u8,
        preference: u32,
    ) -> ExecutionVariant {
        ExecutionVariant {
            variant_id: id.into(),
            task_type: "image.infer".into(),
            runner_id: format!("runner.{id}"),
            plugin_id: "plugin.infer".into(),
            implementation_version: "1.0.0".into(),
            requirements: RequirementSet {
                compute_backends: set(&[backend]),
                precisions: set(&[precision]),
                ..RequirementSet::default()
            },
            quality_level: quality,
            preference,
        }
    }

    #[test]
    fn local_capabilities_select_a_variant_and_report_explicit_degradation() {
        let catalog = ExecutionVariantCatalog {
            variants: vec![
                variant("cuda-fp16", "cuda", "fp16", 100, 0),
                variant("metal-fp16", "metal", "fp16", 100, 1),
                variant("cpu-int8", "cpu", "int8", 60, 2),
            ],
        };
        let policy = ExecutionPolicy {
            latency_class: LatencyClass::Interactive,
            quality: QualityPolicy {
                requested_level: 100,
                minimum_level: 50,
            },
            ..ExecutionPolicy::default()
        };
        let metal = CapabilitySet {
            compute_backends: set(&["metal"]),
            precisions: set(&["fp16"]),
            ..CapabilitySet::default()
        };
        let selected = catalog
            .select_local("image.infer", &metal, &policy)
            .unwrap();
        assert_eq!(selected.variant.variant_id, "metal-fp16");
        assert!(selected.outcome.fallback_used);
        assert!(!selected.outcome.partial);
        assert!(!selected.outcome.stale);

        let cpu = CapabilitySet {
            compute_backends: set(&["cpu"]),
            precisions: set(&["int8"]),
            ..CapabilitySet::default()
        };
        let degraded = catalog.select_local("image.infer", &cpu, &policy).unwrap();
        assert_eq!(degraded.variant.variant_id, "cpu-int8");
        assert_eq!(degraded.outcome.quality_level, 60);
        assert!(
            degraded
                .outcome
                .degradation_reasons
                .contains(&"requested_quality_unavailable".into())
        );
    }

    #[test]
    fn strict_policy_fails_instead_of_silently_degrading() {
        let catalog = ExecutionVariantCatalog {
            variants: vec![variant("cpu-int8", "cpu", "int8", 60, 0)],
        };
        let capabilities = CapabilitySet {
            compute_backends: set(&["cpu"]),
            precisions: set(&["int8"]),
            ..CapabilitySet::default()
        };
        let policy = ExecutionPolicy {
            quality: QualityPolicy {
                requested_level: 100,
                minimum_level: 100,
            },
            no_placement: NoPlacementPolicy::Fail,
            ..ExecutionPolicy::default()
        };

        assert_eq!(
            catalog
                .select_local("image.infer", &capabilities, &policy)
                .unwrap_err()
                .code,
            ERR_EXECUTION_NO_VARIANT
        );
    }

    #[test]
    fn fixed_profile_storage_overwrites_the_ring_and_keeps_bounded_counts() {
        let mut profile = ExecutionProfileAccumulator::new("image.infer", "cpu-int8", "small");
        for sample in 1..=100 {
            profile.record(ExecutionSample {
                latency_ms: sample,
                throughput_per_second: sample as f64,
                peak_memory_bytes: sample * 1024,
                succeeded: sample % 10 != 0,
            });
        }
        let snapshot = profile.snapshot();

        assert_eq!(
            profile.latency_window.len(),
            EXECUTION_PROFILE_WINDOW_CAPACITY
        );
        assert_eq!(profile.latency_histogram.total, 100);
        assert_eq!(snapshot.sample_count, 100);
        assert_eq!(snapshot.peak_memory_bytes, 100 * 1024);
        assert_eq!(snapshot.failure_rate, 0.1);
        assert!(snapshot.p50_ms >= 69);
        assert!(snapshot.p99_ms <= 100);
    }

    #[test]
    fn policy_and_profile_contracts_have_no_deployment_location_fields() {
        let encoded = serde_json::to_value(ExecutionPolicy::default()).unwrap();
        let keys: BTreeSet<_> = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "cache",
                "deadline",
                "failure_mode",
                "latency_class",
                "no_placement",
                "partial_results",
                "quality",
                "scheduling",
            ])
        );
    }
}
