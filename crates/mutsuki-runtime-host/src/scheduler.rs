use std::time::Duration;

use mutsuki_runtime_contracts::{ExecutionClass, RunnerDescriptor};
use mutsuki_runtime_core::{RunnerLoad, RuntimeResult, ScheduleDecision};

#[derive(Clone, Debug)]
pub struct RunnerLimits {
    pub max_running: usize,
    pub max_waiting: usize,
    pub max_inflight: usize,
    pub queue_limit: usize,
    pub deadline_ticks: Option<u64>,
    pub wall_clock_deadline: Option<Duration>,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            max_running: 1,
            max_waiting: 64,
            max_inflight: 64,
            queue_limit: 1024,
            deadline_ticks: None,
            wall_clock_deadline: None,
        }
    }
}

#[derive(Debug)]
pub struct ScheduleInput<'a> {
    pub runner: &'a RunnerDescriptor,
    pub load: &'a RunnerLoad,
    pub limits: &'a RunnerLimits,
    pub pool_slots: usize,
    pub hard_capacity: usize,
    pub current_step: u64,
    pub registry_generation: u64,
}

pub trait SchedulerPolicy: Send + Sync + std::fmt::Debug {
    fn decide(&self, input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision>;
}

#[derive(Clone, Debug, Default)]
pub struct DefaultScheduler;

impl SchedulerPolicy for DefaultScheduler {
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

pub(crate) fn decide_schedule(
    descriptor: &RunnerDescriptor,
    load: &RunnerLoad,
    current_step: u64,
    registry_generation: u64,
    limits: &RunnerLimits,
    pool_slots: usize,
    policy: &dyn SchedulerPolicy,
) -> ScheduleDecision {
    let hard_capacity = hard_dispatch_capacity(descriptor, load, limits, pool_slots);
    let input = ScheduleInput {
        runner: descriptor,
        load,
        limits,
        pool_slots,
        hard_capacity,
        current_step,
        registry_generation,
    };
    let decision = policy.decide(&input).unwrap_or_else(|failure| {
        ScheduleDecision::new(
            "host.default",
            hard_capacity,
            format!("fallback.{}", failure.error().code),
        )
    });
    decision.clamp_to(hard_capacity)
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
    if load.waiting_count >= limits.max_waiting {
        return 0;
    }
    limits
        .max_running
        .saturating_sub(load.running_count)
        .min(limits.max_inflight.saturating_sub(load.pending_weight))
        .min(limits.queue_limit.saturating_sub(load.queued_count))
        .min(pool_slots)
}
