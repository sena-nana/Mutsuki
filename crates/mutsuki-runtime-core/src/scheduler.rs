use mutsuki_runtime_contracts::{StrategyResult, StrategyResultStatus};

use crate::{AgentRuntime, RuntimeBackend, RuntimeResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerOptions {
    pub max_ticks: usize,
    pub stop_on_wait_input: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerDecision {
    Continue,
    Halt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerStopReason {
    Completed,
    Failed,
    Halted,
    MaxTicks,
    WaitInput,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeTickOutcome {
    pub strategy: StrategyResult,
    pub operation: Option<crate::BackendPayload>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerReport {
    pub ticks: usize,
    pub stop_reason: SchedulerStopReason,
}

pub trait SchedulerDriver<B: RuntimeBackend> {
    fn before_tick(
        &mut self,
        _runtime: &mut AgentRuntime,
        _agent_id: &str,
        _backend: &mut B,
    ) -> RuntimeResult<SchedulerDecision> {
        Ok(SchedulerDecision::Continue)
    }

    fn after_tick(
        &mut self,
        _runtime: &mut AgentRuntime,
        _agent_id: &str,
        _backend: &mut B,
        _outcome: &RuntimeTickOutcome,
    ) -> RuntimeResult<SchedulerDecision> {
        Ok(SchedulerDecision::Continue)
    }
}

pub struct AgentScheduler {
    options: SchedulerOptions,
}

impl AgentScheduler {
    pub fn new(options: SchedulerOptions) -> Self {
        Self { options }
    }

    pub fn run_with_driver<B, D>(
        &self,
        runtime: &mut AgentRuntime,
        agent_id: &str,
        backend: &mut B,
        driver: &mut D,
    ) -> RuntimeResult<SchedulerReport>
    where
        B: RuntimeBackend,
        D: SchedulerDriver<B>,
    {
        for tick in 0..self.options.max_ticks {
            if driver.before_tick(runtime, agent_id, backend)? == SchedulerDecision::Halt {
                return Ok(SchedulerReport {
                    ticks: tick,
                    stop_reason: SchedulerStopReason::Halted,
                });
            }
            let outcome = runtime.tick_once_and_drive(agent_id, backend)?;
            let stop_reason = stop_reason_for_outcome(&outcome, self.options.stop_on_wait_input);
            let decision = driver.after_tick(runtime, agent_id, backend, &outcome)?;
            if decision == SchedulerDecision::Halt {
                return Ok(SchedulerReport {
                    ticks: tick + 1,
                    stop_reason: SchedulerStopReason::Halted,
                });
            }
            if let Some(stop_reason) = stop_reason {
                return Ok(SchedulerReport {
                    ticks: tick + 1,
                    stop_reason,
                });
            }
        }
        Ok(SchedulerReport {
            ticks: self.options.max_ticks,
            stop_reason: SchedulerStopReason::MaxTicks,
        })
    }
}

fn stop_reason_for_outcome(
    outcome: &RuntimeTickOutcome,
    stop_on_wait_input: bool,
) -> Option<SchedulerStopReason> {
    if outcome.strategy.error.is_some() {
        return Some(SchedulerStopReason::Failed);
    }
    match outcome.strategy.status {
        StrategyResultStatus::Completed => Some(SchedulerStopReason::Completed),
        StrategyResultStatus::Failed => Some(SchedulerStopReason::Failed),
        StrategyResultStatus::WaitInput if stop_on_wait_input => {
            Some(SchedulerStopReason::WaitInput)
        }
        _ => None,
    }
}
