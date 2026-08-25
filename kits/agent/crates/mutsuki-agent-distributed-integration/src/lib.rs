// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::default_trait_access,
    clippy::elidable_lifetime_names,
    clippy::if_not_else,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls
)]

use std::collections::BTreeSet;

use mutsuki_agent_contracts::{AgentAffinity, AgentTaskPlacement};
use mutsuki_distributed_contracts::{
    CapabilityBits, LatencyClass, LexicographicPriority, NodeId, PlacementFlags, PlacementSlo,
    QualityPolicy, TaskPlacementRequest, WorkOrigin,
};

mod database;
mod recovery;
mod remote;

pub use database::*;
pub use recovery::*;
pub use remote::*;

pub trait AgentCapabilityResolver {
    fn resolve(&self, capabilities: &[String]) -> Result<CapabilityBits, String>;
}

#[allow(clippy::too_many_arguments)]
pub fn distributed_request(
    task_id: impl Into<String>,
    task_type: impl Into<String>,
    placement: &AgentTaskPlacement,
    capabilities: &dyn AgentCapabilityResolver,
    local_node: NodeId,
    input_bytes: u64,
    output_bytes: u64,
    local_estimated_cost: f64,
) -> Result<TaskPlacementRequest, String> {
    let flags = if placement.permits_remote_execution() {
        PlacementFlags::default()
    } else {
        PlacementFlags::LOCAL_ONLY
    };
    let session_node = match &placement.affinity {
        AgentAffinity::None => None,
        AgentAffinity::Preferred(node) | AgentAffinity::Required(node) => {
            Some(NodeId(node.clone()))
        }
    };
    Ok(TaskPlacementRequest {
        task_id: task_id.into(),
        task_type: task_type.into(),
        input_bucket: input_bytes.ilog2().min(u32::from(u8::MAX)) as u8,
        local_node,
        priority: LexicographicPriority {
            safety_critical: false,
            recovery_critical: false,
            latency_class: parse_latency_class(&placement.latency_class),
            deadline_risk: 0,
            dag_criticality: 0,
            unlock_value: 0,
            business_priority: 0,
            age_ticks: 0,
            fair_share_credit: 0,
            origin: WorkOrigin::Local,
        },
        required_capabilities: capabilities.resolve(&placement.required_capabilities)?,
        required_os: None,
        required_abi: None,
        minimum_trust: 0,
        required_memory_bytes: input_bytes.saturating_add(output_bytes),
        required_vram_bytes: 0,
        required_plugin: None,
        required_content: placement
            .data_locality
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        flags,
        input_bytes,
        output_bytes,
        local_estimated_cost,
        safety_margin: 1.0,
        small_task_threshold: 0.0,
        quality_policy: QualityPolicy::Exact,
        session_node,
        migration_cost: if matches!(placement.affinity, AgentAffinity::Required(_)) {
            f64::MAX
        } else {
            1.0
        },
        dag_cross_node_cost: 0.0,
        dag_parallel_benefit: 0.0,
        slo: PlacementSlo {
            deadline_ticks: f64::MAX,
            max_p95_ticks: f64::MAX,
            max_p99_ticks: f64::MAX,
            max_jitter_ticks: f64::MAX,
            max_failure_probability: 1.0,
            minimum_quality: 0.0,
            streaming: false,
            max_ttft_ticks: None,
            max_steady_latency_ticks: None,
        },
    })
}

fn parse_latency_class(value: &str) -> LatencyClass {
    match value {
        "hard_realtime" => LatencyClass::HardRealtime,
        "soft_realtime" => LatencyClass::SoftRealtime,
        "interactive" => LatencyClass::Interactive,
        "cluster_safety" => LatencyClass::ClusterSafety,
        _ => LatencyClass::Batch,
    }
}
