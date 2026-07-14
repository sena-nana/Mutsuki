use std::time::Duration;

use mutsuki_runtime_contracts::{ExecutionClass, RunnerDescriptor};
use mutsuki_runtime_core::{RunnerLoad, RuntimeResult, ScheduleDecision};

#[derive(Clone, Debug)]
pub struct RunnerLimits {
    pub max_running: usize,
    pub max_waiting: usize,
    pub max_inflight: usize,
    pub deadline_ticks: Option<u64>,
    pub wall_clock_deadline: Option<Duration>,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            max_running: 1,
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
        Ok(ScheduleDecision::new(
            "host.default",
            input.hard_capacity,
            reason,
        ))
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
    running_batches: usize,
    policy: &dyn SchedulerPolicy,
) -> RuntimeResult<ScheduleDecision> {
    let hard_capacity = hard_dispatch_capacity(descriptor, load, limits, pool_slots);
    let host_capacity = host_capacity(descriptor, load, limits, running_batches);
    let input = ScheduleInput {
        runner: descriptor,
        load,
        limits,
        host_capacity,
        pool_slots,
        hard_capacity,
        current_step,
        registry_generation,
    };
    let decision = policy.decide(&input)?;
    Ok(decision.clamp_to(hard_capacity))
}

fn host_capacity(
    descriptor: &RunnerDescriptor,
    load: &RunnerLoad,
    limits: &RunnerLimits,
    running_batches: usize,
) -> HostCapacity {
    let max_inflight = limits.max_inflight.max(1);
    HostCapacity {
        running_batches,
        queued_batches: load.queued_count,
        running_entries: load.running_count,
        queued_entries: load.queued_count,
        saturation: (load.pending_weight as f32 / max_inflight as f32).min(1.0),
        preferred_batch_size: descriptor.batch.preferred_batch_size.max(1),
        max_entry_concurrency: descriptor
            .batch
            .max_entry_concurrency
            .min(descriptor.batch.max_batch_entries)
            .min(limits.max_inflight.max(1)),
        max_inflight_bytes: usize::MAX,
    }
}

fn hard_dispatch_capacity(
    descriptor: &RunnerDescriptor,
    load: &RunnerLoad,
    limits: &RunnerLimits,
    pool_slots: usize,
) -> usize {
    if descriptor.execution_class == ExecutionClass::Control {
        return if descriptor.runner_id == "core.kernel" {
            1
        } else {
            0
        };
    }
    standard_dispatch_capacity(load, limits, pool_slots)
}

fn standard_dispatch_capacity(
    load: &RunnerLoad,
    limits: &RunnerLimits,
    pool_slots: usize,
) -> usize {
    if load.waiting_count >= limits.max_waiting {
        return 0;
    }
    let inflight = load.running_count.saturating_add(load.waiting_count);
    limits
        .max_running
        .saturating_sub(load.running_count)
        .min(limits.max_inflight.saturating_sub(inflight))
        .min(pool_slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_backlog_does_not_consume_inflight_capacity() {
        let load = RunnerLoad {
            running_count: 0,
            waiting_count: 0,
            queued_count: 1_024,
            pending_weight: 1_024,
        };
        let limits = RunnerLimits {
            max_running: 4,
            max_inflight: 4,
            ..RunnerLimits::default()
        };

        assert_eq!(standard_dispatch_capacity(&load, &limits, 1_024), 4);
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
            max_running: 4,
            max_inflight: 2,
            ..RunnerLimits::default()
        };

        assert_eq!(standard_dispatch_capacity(&load, &limits, 4), 0);
    }
}
