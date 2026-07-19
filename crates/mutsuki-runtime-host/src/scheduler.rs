use std::time::Duration;

use mutsuki_runtime_contracts::{ExecutionClass, RunnerDescriptor, RuntimeError, ScalarValue};
use mutsuki_runtime_core::{
    DispatchBudget, RunnerLoad, RuntimeFailure, RuntimeResult, ScheduleDecision,
};

use crate::worker::PoolCapacitySnapshot;

#[derive(Clone, Debug)]
pub struct RunnerLimits {
    /// Maximum active batches for one logical runner id.
    ///
    /// Descriptor concurrency and Host capacity still clamp this value.
    pub max_running: usize,
    pub max_waiting: usize,
    pub max_inflight: usize,
    pub deadline_ticks: Option<u64>,
    pub wall_clock_deadline: Option<Duration>,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            max_running: 64,
            max_waiting: 64,
            max_inflight: 64,
            deadline_ticks: None,
            wall_clock_deadline: None,
        }
    }
}

#[derive(Debug)]
pub struct HostCapacity {
    pub running_batches: usize,
    pub queued_batches: usize,
    pub max_running_batches: usize,
    pub running_entries: usize,
    pub queued_entries: usize,
    pub saturation: f32,
    pub preferred_batch_size: usize,
    pub max_entry_concurrency: usize,
    pub max_inflight_bytes: usize,
}

#[derive(Debug)]
pub struct ScheduleInput<'a> {
    pub runner: &'a RunnerDescriptor,
    pub load: &'a RunnerLoad,
    pub limits: &'a RunnerLimits,
    pub host_capacity: HostCapacity,
    pub pool_slots: usize,
    pub hard_capacity: usize,
    pub hard_batch_capacity: usize,
    pub current_step: u64,
    pub registry_generation: u64,
}

pub trait SchedulerPolicy: Send + Sync + std::fmt::Debug {
    fn policy_id(&self) -> &str {
        DefaultScheduler::POLICY_ID
    }

    fn decide(&self, input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision>;
}

#[derive(Clone, Debug, Default)]
pub struct DefaultScheduler;

impl DefaultScheduler {
    pub const POLICY_ID: &'static str = "host.default";
}

impl SchedulerPolicy for DefaultScheduler {
    fn policy_id(&self) -> &str {
        Self::POLICY_ID
    }

    fn decide(&self, input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision> {
        let reason = if input.hard_capacity == 0 {
            "capacity.exhausted"
        } else {
            "capacity.available"
        };
        Ok(
            ScheduleDecision::new("host.default", input.hard_capacity, reason).with_budget(
                DispatchBudget {
                    max_entries: input.hard_capacity,
                    max_batches: input.hard_batch_capacity,
                    max_bytes: usize::MAX,
                    lane_budget: Default::default(),
                },
            ),
        )
    }
}

// Inputs remain explicit because each value is an independently clamped scheduling fact.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_schedule(
    descriptor: &RunnerDescriptor,
    load: &RunnerLoad,
    current_step: u64,
    registry_generation: u64,
    limits: &RunnerLimits,
    pool_slots: usize,
    pool_capacity: PoolCapacitySnapshot,
    running_batches: usize,
    policy: &dyn SchedulerPolicy,
) -> RuntimeResult<ScheduleDecision> {
    let (hard_capacity, hard_batch_capacity) =
        hard_dispatch_capacity(descriptor, load, limits, pool_slots, running_batches);
    let host_capacity = host_capacity(descriptor, limits, pool_capacity);
    let input = ScheduleInput {
        runner: descriptor,
        load,
        limits,
        host_capacity,
        pool_slots,
        hard_capacity,
        hard_batch_capacity,
        current_step,
        registry_generation,
    };
    let mut decision = policy
        .decide(&input)?
        .clamp_to(hard_capacity)
        .clamp_batches(hard_batch_capacity);
    decision.budget.max_bytes = decision.budget.max_bytes.min(
        pool_capacity
            .max_inflight_bytes
            .saturating_sub(pool_capacity.inflight_bytes),
    );
    Ok(decision)
}

fn host_capacity(
    descriptor: &RunnerDescriptor,
    limits: &RunnerLimits,
    pool: PoolCapacitySnapshot,
) -> HostCapacity {
    let max_inflight_entries = limits.max_inflight.max(1);
    HostCapacity {
        running_batches: pool.running_batches,
        queued_batches: pool.queued_batches,
        max_running_batches: pool.active_threads,
        running_entries: pool.running_entries,
        queued_entries: pool.queued_entries,
        saturation: ((pool.running_entries + pool.queued_entries) as f32
            / max_inflight_entries as f32)
            .min(1.0),
        preferred_batch_size: descriptor.batch.preferred_batch_size.max(1),
        max_entry_concurrency: descriptor
            .batch
            .max_entry_concurrency
            .min(descriptor.batch.max_batch_entries)
            .min(limits.max_inflight.max(1)),
        max_inflight_bytes: pool.max_inflight_bytes.saturating_sub(pool.inflight_bytes),
    }
}

fn hard_dispatch_capacity(
    descriptor: &RunnerDescriptor,
    load: &RunnerLoad,
    limits: &RunnerLimits,
    pool_slots: usize,
    running_batches: usize,
) -> (usize, usize) {
    if descriptor.execution_class == ExecutionClass::Control {
        return if descriptor.runner_id == "core.kernel" {
            (1, 1)
        } else {
            (0, 0)
        };
    }
    standard_dispatch_capacity(
        descriptor,
        load,
        limits,
        pool_slots,
        running_batches,
        descriptor.batch.max_batch_entries,
    )
}

fn standard_dispatch_capacity(
    descriptor: &RunnerDescriptor,
    load: &RunnerLoad,
    limits: &RunnerLimits,
    pool_slots: usize,
    running_batches: usize,
    max_batch_entries: usize,
) -> (usize, usize) {
    let maximum_batches = descriptor
        .concurrency
        .max_inflight_batches()
        .min(limits.max_running);
    let available_batches = maximum_batches
        .saturating_sub(running_batches)
        .min(pool_slots);
    let exclusive_blocked = matches!(
        descriptor.concurrency,
        mutsuki_runtime_contracts::RunnerConcurrency::Exclusive
    ) && load.running_count.saturating_add(load.waiting_count) > 0;
    if available_batches == 0 || exclusive_blocked || load.waiting_count >= limits.max_waiting {
        return (0, 0);
    }
    let inflight = load.running_count.saturating_add(load.waiting_count);
    let descriptor_entries = match descriptor.concurrency {
        mutsuki_runtime_contracts::RunnerConcurrency::Reentrant {
            max_inflight_entries,
            ..
        } => max_inflight_entries,
        _ => max_batch_entries.saturating_mul(maximum_batches),
    };
    let available_entries = descriptor_entries
        .min(limits.max_inflight)
        .saturating_sub(inflight)
        .min(max_batch_entries.saturating_mul(available_batches));
    if available_entries == 0 {
        (0, 0)
    } else {
        (available_entries, available_batches.min(available_entries))
    }
}

pub(crate) fn validate_runner_limits(
    default_limits: &RunnerLimits,
    runner_limits: &std::collections::BTreeMap<String, RunnerLimits>,
) -> RuntimeResult<()> {
    validate_runner_limit("default", default_limits)?;
    for (runner_id, limits) in runner_limits {
        validate_runner_limit(runner_id, limits)?;
    }
    Ok(())
}

fn validate_runner_limit(scope: &str, limits: &RunnerLimits) -> RuntimeResult<()> {
    if limits.max_running > 0 && limits.max_inflight > 0 {
        return Ok(());
    }
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "runtime.host",
        format!("host.runner_limits.{scope}.max_running"),
    );
    error.evidence.insert(
        "configured_max_running".into(),
        ScalarValue::Int(limits.max_running as i64),
    );
    error.evidence.insert(
        "configured_max_inflight".into(),
        ScalarValue::Int(limits.max_inflight as i64),
    );
    Err(RuntimeFailure::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> RunnerDescriptor {
        RunnerDescriptor {
            runner_id: "test.runner".into(),
            plugin_id: "test.plugin".into(),
            plugin_generation: 1,
            accepted_protocol_ids: vec!["test.work".into()],
            purity: mutsuki_runtime_contracts::RunnerPurity::Pure,
            execution_class: mutsuki_runtime_contracts::ExecutionClass::Cpu,
            invocation_mode: mutsuki_runtime_contracts::InvocationMode::SyncExclusive,
            concurrency: mutsuki_runtime_contracts::RunnerConcurrency::Exclusive,
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            batch: Default::default(),
            payload: Default::default(),
            resources: Default::default(),
            ordering: Default::default(),
            control: Default::default(),
            metadata: Default::default(),
            contract_surfaces: vec!["runner:test.runner".into()],
        }
    }

    #[test]
    fn ready_backlog_does_not_consume_inflight_capacity() {
        let load = RunnerLoad {
            running_count: 0,
            waiting_count: 0,
            queued_count: 1_024,
            pending_weight: 1_024,
        };
        let limits = RunnerLimits {
            max_running: 1,
            max_inflight: 4,
            ..RunnerLimits::default()
        };

        assert_eq!(
            standard_dispatch_capacity(&descriptor(), &load, &limits, 1_024, 0, 64),
            (4, 1)
        );
    }

    #[test]
    fn running_and_waiting_consume_inflight_capacity() {
        let load = RunnerLoad {
            running_count: 1,
            waiting_count: 1,
            queued_count: 1,
            pending_weight: 3,
        };
        let limits = RunnerLimits {
            max_running: 1,
            max_inflight: 2,
            ..RunnerLimits::default()
        };

        assert_eq!(
            standard_dispatch_capacity(&descriptor(), &load, &limits, 4, 0, 4),
            (0, 0)
        );
    }

    #[test]
    fn active_batch_exhausts_single_runner_capacity() {
        let load = RunnerLoad {
            running_count: 4,
            waiting_count: 0,
            queued_count: 4,
            pending_weight: 8,
        };

        assert_eq!(
            standard_dispatch_capacity(&descriptor(), &load, &RunnerLimits::default(), 4, 1, 4,),
            (0, 0)
        );
    }
}
