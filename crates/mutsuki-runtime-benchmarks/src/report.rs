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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseResult {
    pub id: String,
    pub category: String,
    pub dimensions: BTreeMap<String, String>,
    pub iterations: u64,
    pub units: u64,
    pub elapsed_ns: u64,
    pub ns_per_unit: f64,
    pub throughput_per_second: f64,
    pub allocations: AllocationMetrics,
    pub counters: BTreeMap<String, i128>,
}

impl CaseResult {
    // Keep measurement facts explicit at call sites so units cannot be confused
    // with iterations or elapsed time when adding a benchmark case.
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
            elapsed_ns,
            ns_per_unit: elapsed_ns as f64 / denominator,
            throughput_per_second: denominator * 1_000_000_000.0 / elapsed_ns.max(1) as f64,
            allocations,
            counters,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub os: String,
    pub arch: String,
    pub cpu_parallelism: usize,
    pub rust_version: String,
    pub commit: String,
    pub dirty: bool,
    pub profile: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateResult {
    pub name: String,
    pub kind: String,
    pub passed: bool,
    pub actual: f64,
    pub limit: f64,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub issue: u32,
    pub mode: String,
    pub generated_unix_seconds: u64,
    pub command: String,
    pub environment: Environment,
    pub cases: Vec<CaseResult>,
    pub gates: Vec<GateResult>,
    pub passed: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BaselineAllocation {
    pub allocated_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BaselineCase {
    pub id: String,
    pub units: u64,
    pub ns_per_unit: f64,
    pub allocations: BaselineAllocation,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BaselineReport {
    pub cases: Vec<BaselineCase>,
}
