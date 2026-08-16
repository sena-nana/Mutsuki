//! Event-driven tick planning, lane QoS, capacity reservation, and dispatch.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    AsyncInvocation, CancelPolicy, DispatchLane, ExecutionClass, InvocationMode, RunnerId,
    TaskHandle, TaskId,
};
use mutsuki_runtime_core::{
    CoreRuntime, RunnerCompletion, RunnerDispatchTarget, RunnerLoopReport, RuntimeResult,
};

use crate::error::host_failure;
use crate::host::{HostRuntimeConfig, HostRuntimeDriveState};
use crate::scheduler::{apply_lane_qos, decide_schedule};
use crate::worker::{WorkerDispatchError, WorkerPools};

use super::supervision::RunningBatch;
#[derive(Default)]
/// Event-driven wake plan derived from core steps and invocation deadlines.
pub(super) struct DriverState {
    pub(super) scheduled_tick: Option<(u64, Instant)>,
    pub(super) timed_wakeups: u64,
}

impl DriverState {
    pub(super) fn refresh_scheduled_tick(
        &mut self,
        target_step: Option<u64>,
        current_step: u64,
        tick_interval: Duration,
    ) {
        if target_step == self.scheduled_tick.map(|(target_step, _)| target_step) {
            return;
        }
        self.scheduled_tick = target_step.and_then(|target_step| {
            let remaining_steps = target_step.saturating_sub(current_step);
            let total_nanos = tick_interval
                .as_nanos()
                .checked_mul(remaining_steps as u128)?;
            let duration = Duration::new(
                u64::try_from(total_nanos / 1_000_000_000).ok()?,
                (total_nanos % 1_000_000_000) as u32,
            );
            Some((target_step, Instant::now().checked_add(duration)?))
        });
    }

    pub(super) fn next_wake_deadline(
        &self,
        config: &HostRuntimeConfig,
        running_batches_by_task: &BTreeMap<TaskId, RunningBatch>,
    ) -> Option<Instant> {
        if !config.event_driven {
            return None;
        }
        self.scheduled_tick
            .map(|(_, deadline)| deadline)
            .into_iter()
            .chain(running_batches_by_task.values().flat_map(|task| {
                [
                    task.wall_clock_deadline_at,
                    task.cancel_requested_at.and_then(|instant| {
                        config
                            .cancel_grace_period
                            .and_then(|grace| instant.checked_add(grace))
                    }),
                    task.worker_started_at.and_then(|instant| {
                        config
                            .worker_health_timeout
                            .and_then(|timeout| instant.checked_add(timeout))
                    }),
                ]
                .into_iter()
                .flatten()
            }))
            .min()
    }

    pub(super) fn snapshot(
        &self,
        core: &CoreRuntime,
        config: &HostRuntimeConfig,
        running_batches_by_task: &BTreeMap<TaskId, RunningBatch>,
    ) -> HostRuntimeDriveState {
        HostRuntimeDriveState {
            current_step: core.current_step(),
            next_required_tick: next_required_tick(core, running_batches_by_task),
            next_wake_deadline: self.next_wake_deadline(config, running_batches_by_task),
            timed_wakeups: self.timed_wakeups,
        }
    }
}

pub(super) fn next_required_tick(
    core: &CoreRuntime,
    running_batches_by_task: &BTreeMap<TaskId, RunningBatch>,
) -> Option<u64> {
    let current_step = core.current_step();
    core.next_required_step()
        .into_iter()
        .chain(
            running_batches_by_task
                .values()
                .filter_map(|task| task.deadline_tick)
                .filter(|step| *step > current_step),
        )
        .min()
}

fn task_handle(task: &mutsuki_runtime_contracts::Task) -> TaskHandle {
    TaskHandle {
        task_id: task.task_id.clone(),
        protocol_id: task.protocol_id.clone(),
        target_binding_id: task.target_binding_id.clone(),
        cancel_policy: CancelPolicy::Cascade,
        trace_id: task.trace_id.clone(),
        correlation_id: task.correlation_id.clone(),
    }
}

fn running_batch_count_for_runner(
    running_batches_by_task: &BTreeMap<TaskId, RunningBatch>,
    runner_id: &RunnerId,
) -> usize {
    running_batches_by_task
        .values()
        .filter(|task| &task.runner_id == runner_id)
        .map(|task| task.batch_id.clone())
        .collect::<BTreeSet<_>>()
        .len()
}

pub(super) fn schedule_ready(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    running_batches_by_task: &mut BTreeMap<TaskId, RunningBatch>,
) -> RuntimeResult<RunnerLoopReport> {
    let target_step = core.current_step().saturating_add(1);
    schedule_ready_at(target_step, core, config, pools, running_batches_by_task)
}

/// Claims through `CoreRuntime`, reserves physical capacity, then dispatches to
/// the selected bounded worker or async executor without becoming a fact source.
pub(super) fn schedule_ready_at(
    target_step: u64,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    running_batches_by_task: &mut BTreeMap<TaskId, RunningBatch>,
) -> RuntimeResult<RunnerLoopReport> {
    let scheduler_started = Instant::now();
    let mut domain_lane_demand: BTreeMap<String, BTreeMap<DispatchLane, usize>> = BTreeMap::new();
    for (descriptor, load) in core.runner_load_snapshot() {
        let async_invocation = matches!(
            descriptor.invocation_mode,
            InvocationMode::AsyncReentrant | InvocationMode::AsyncExclusive
        );
        let domain_id = if async_invocation {
            "async".to_string()
        } else {
            pools
                .domain_id(&descriptor.execution_class)
                .unwrap_or("control")
                .to_string()
        };
        let demand = domain_lane_demand.entry(domain_id).or_default();
        for (lane, count) in load.queued_by_lane {
            *demand.entry(lane).or_default() += count;
        }
    }
    let mut domain_reservations: BTreeMap<String, usize> = BTreeMap::new();
    let mut async_reservations = 0usize;
    let (report, dispatches) = core.claim_ready_dispatches_at_step(
        target_step,
        |descriptor, load, current_step, registry_generation| {
            let limits = config
                .runner_limits
                .get(descriptor.runner_id.as_str())
                .unwrap_or(&config.default_runner_limits);
            let async_invocation = matches!(
                descriptor.invocation_mode,
                InvocationMode::AsyncReentrant | InvocationMode::AsyncExclusive
            );
            let reservations = if async_invocation {
                &mut async_reservations
            } else {
                if descriptor.execution_class == ExecutionClass::Control {
                    if descriptor.runner_id.as_str() == "core.kernel" {
                        return Ok(mutsuki_runtime_core::ScheduleDecision::new(
                            "host.default",
                            1,
                            "control.kernel.inline",
                        ));
                    }
                    return Ok(mutsuki_runtime_core::ScheduleDecision::new(
                        "host.default",
                        0,
                        "control.inline",
                    ));
                }
                let domain_id = pools
                    .domain_id(&descriptor.execution_class)
                    .ok_or_else(|| {
                        host_failure(
                            "host.execution_domain.class_missing",
                            format!("execution_class.{:?}", descriptor.execution_class),
                        )
                    })?
                    .to_string();
                domain_reservations.entry(domain_id).or_default()
            };
            let (pool_slots, mut pool_capacity) = if async_invocation {
                if let Some(executor) = &config.async_executor {
                    let snapshot = executor.snapshot();
                    (
                        snapshot
                            .max_inflight_invocations
                            .saturating_sub(snapshot.running_invocations),
                        crate::worker::PoolCapacitySnapshot {
                            active_threads: snapshot.max_inflight_invocations,
                            queued_batches: 0,
                            queued_entries: 0,
                            running_batches: snapshot.running_invocations,
                            running_entries: snapshot.running_entries,
                            inflight_bytes: snapshot.inflight_bytes,
                            max_inflight_bytes: snapshot.max_inflight_bytes,
                        },
                    )
                } else {
                    (
                        descriptor.concurrency.max_inflight_batches(),
                        crate::worker::PoolCapacitySnapshot {
                            active_threads: descriptor.concurrency.max_inflight_batches(),
                            max_inflight_bytes: usize::MAX,
                            ..Default::default()
                        },
                    )
                }
            } else {
                pools
                    .get(&descriptor.execution_class)
                    .map(|pool| (pool.available_slots(), pool.capacity()))
                    .unwrap_or_default()
            };
            let pool_slots = pool_slots.saturating_sub(*reservations);
            pool_capacity.queued_batches =
                pool_capacity.queued_batches.saturating_add(*reservations);
            let running_batches =
                running_batch_count_for_runner(running_batches_by_task, &descriptor.runner_id);
            let mut decision = decide_schedule(
                descriptor,
                load,
                current_step,
                registry_generation,
                limits,
                pool_slots,
                pool_capacity,
                running_batches,
                config.scheduler_policy.as_ref(),
            )?;
            if !async_invocation
                && let Some(domain_id) = pools.domain_id(&descriptor.execution_class)
                && let Some(policies) = pools.lane_policies(&descriptor.execution_class)
                && let Some(demand) = domain_lane_demand.get(domain_id)
            {
                decision = apply_lane_qos(decision, load, demand, policies);
                let starvation = demand
                    .iter()
                    .filter(|(lane, count)| {
                        **count > 0
                            && decision
                                .budget
                                .lane_budget
                                .get(*lane)
                                .is_some_and(|budget| budget.max_entries == 0)
                    })
                    .count();
                config.actor_metrics.record_lane_starvation(starvation);
                let reserved_uses = [DispatchLane::Control, DispatchLane::Interactive]
                    .into_iter()
                    .filter(|lane| {
                        demand.get(lane).copied().unwrap_or_default() > 0
                            && decision
                                .budget
                                .lane_budget
                                .get(lane)
                                .is_some_and(|budget| budget.max_entries > 0)
                    })
                    .count();
                config
                    .actor_metrics
                    .record_reserved_capacity_use(reserved_uses);
            }
            if decision.dispatch_limit > 0 && decision.budget.max_batches > 0 {
                *reservations = (*reservations).saturating_add(decision.budget.max_batches);
            }
            Ok(decision)
        },
        None,
    )?;
    let mut deferred_entries = 0usize;
    let mut rejected_entries = 0usize;
    for mut dispatch in dispatches {
        let execution_class = dispatch.target.descriptor().execution_class.clone();
        let runner_id = dispatch.target.descriptor().runner_id.clone();
        let limits = config
            .runner_limits
            .get(runner_id.as_str())
            .unwrap_or(&config.default_runner_limits);
        dispatch.ctx.deadline_tick = limits
            .deadline_ticks
            .map(|ticks| dispatch.ctx.current_step.saturating_add(ticks));
        dispatch.ctx.deadline_after_ms = limits
            .wall_clock_deadline
            .and_then(|deadline| u64::try_from(deadline.as_millis()).ok());
        let invocation_id = dispatch.ctx.invocation_id.clone();
        let batch_id = dispatch.ctx.batch_id.clone();
        let isolation = dispatch.target.isolation();
        let management = dispatch.target.management_handle();
        let deadline_tick = dispatch.ctx.deadline_tick;
        let wall_clock_deadline_at = limits
            .wall_clock_deadline
            .map(|deadline| Instant::now() + deadline);
        let tasks = dispatch
            .batch
            .row_payload_tasks()
            .map_err(mutsuki_runtime_core::RuntimeFailure::new)?;
        if dispatch.target.is_async() {
            let Some(executor) = config.async_executor.as_ref() else {
                let batch_id = dispatch.batch.batch_id.clone();
                let expected_entries = dispatch.batch.entries.clone();
                rejected_entries = rejected_entries.saturating_add(
                    core.complete_runner_dispatch(RunnerCompletion {
                        runner: None,
                        task_leases: dispatch.task_leases,
                        batch_id,
                        expected_entries,
                        result: Err(host_failure(
                            "host.async_executor.unavailable",
                            format!("runner {runner_id} requires an async executor"),
                        )),
                    })?
                    .completed_tasks,
                );
                continue;
            };
            let Some(events) = config.async_event_sink.clone() else {
                return Err(host_failure(
                    "host.async_executor.event_sink",
                    "async executor event sink is not configured",
                ));
            };
            let payload_bytes = serde_json::to_vec(&dispatch.batch.payload)
                .map_err(|error| host_failure("host.async_executor.payload", error.to_string()))?
                .len();
            let invocation = AsyncInvocation {
                invocation_id: invocation_id.clone(),
                batch_id: batch_id.clone(),
                runner_id: runner_id.clone(),
                task_ids: dispatch
                    .task_leases
                    .iter()
                    .map(|lease| lease.task_id.clone())
                    .collect(),
                task_lease_ids: dispatch
                    .task_leases
                    .iter()
                    .map(|lease| lease.lease_id.clone())
                    .collect(),
                attempt_generations: dispatch
                    .task_leases
                    .iter()
                    .map(|lease| lease.attempt_generation)
                    .collect(),
                task_leases: dispatch.task_leases.clone(),
                expected_entries: dispatch.batch.entries.clone(),
                registry_generation: dispatch.ctx.registry_generation,
                plugin_generation: dispatch.target.descriptor().plugin_generation,
                cancel_token: dispatch.ctx.cancel_token.clone(),
                deadline_after_ms: dispatch.ctx.deadline_after_ms,
                entry_count: dispatch.batch.entries.len(),
                payload_bytes,
            };
            let pending_task_leases = dispatch.task_leases.clone();
            let pending_entries = dispatch.batch.entries.clone();
            let RunnerDispatchTarget::Async(handler) = dispatch.target else {
                unreachable!("async dispatch target checked above")
            };
            let future = handler.run_batch(dispatch.ctx, dispatch.batch);
            let async_handle = match executor.spawn(invocation, future, events) {
                Ok(handle) => handle,
                Err(failure) => {
                    rejected_entries = rejected_entries.saturating_add(
                        core.complete_runner_dispatch(RunnerCompletion {
                            runner: None,
                            task_leases: pending_task_leases,
                            batch_id: batch_id.clone(),
                            expected_entries: pending_entries,
                            result: Err(failure),
                        })?
                        .completed_tasks,
                    );
                    continue;
                }
            };
            for task in tasks {
                let handle = task_handle(&task);
                running_batches_by_task.insert(
                    task.task_id.clone(),
                    RunningBatch {
                        runner_id: runner_id.clone(),
                        invocation_id: invocation_id.clone(),
                        batch_id: batch_id.clone(),
                        execution_class: execution_class.clone(),
                        handle,
                        deadline_tick,
                        wall_clock_deadline_at,
                        cancel_requested_at: None,
                        worker_id: Some("async_io".into()),
                        worker_started_at: None,
                        isolation: isolation.clone(),
                        management: management.clone(),
                        async_handle: Some(async_handle.clone()),
                    },
                );
            }
            continue;
        }
        let Some(pool) = pools.get(&execution_class) else {
            return Err(host_failure(
                "host.worker.pool_missing",
                format!("execution_class.{execution_class:?}"),
            ));
        };
        if let Err(error) = pool.send(dispatch) {
            let WorkerDispatchError {
                failure,
                dispatch,
                retryable,
            } = error;
            let dispatch = *dispatch;
            if retryable {
                deferred_entries =
                    deferred_entries.saturating_add(core.defer_runner_dispatch(dispatch)?);
            } else {
                let batch_id = dispatch.batch.batch_id.clone();
                let expected_entries = dispatch.batch.entries.clone();
                rejected_entries = rejected_entries.saturating_add(
                    core.complete_runner_dispatch(RunnerCompletion {
                        runner: match dispatch.target {
                            RunnerDispatchTarget::Sync(runner) => Some(runner),
                            RunnerDispatchTarget::Async(_) => None,
                        },
                        task_leases: dispatch.task_leases,
                        batch_id,
                        expected_entries,
                        result: Err(failure),
                    })?
                    .completed_tasks,
                );
            }
            continue;
        }
        for task in tasks {
            let handle = task_handle(&task);
            running_batches_by_task.insert(
                task.task_id.clone(),
                RunningBatch {
                    runner_id: runner_id.clone(),
                    invocation_id: invocation_id.clone(),
                    batch_id: batch_id.clone(),
                    execution_class: execution_class.clone(),
                    handle,
                    deadline_tick,
                    wall_clock_deadline_at,
                    cancel_requested_at: None,
                    worker_id: None,
                    worker_started_at: None,
                    isolation: isolation.clone(),
                    management: management.clone(),
                    async_handle: None,
                },
            );
        }
    }
    let report = RunnerLoopReport {
        claimed_tasks: report.claimed_tasks.saturating_sub(deferred_entries),
        completed_tasks: report.completed_tasks.saturating_add(rejected_entries),
    };
    config
        .actor_metrics
        .record_scheduler_pass(scheduler_started.elapsed());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_core::RunnerIsolation;

    fn running_batch(runner_id: &str, batch_id: &str, task_id: &str) -> RunningBatch {
        RunningBatch {
            runner_id: runner_id.into(),
            invocation_id: batch_id.into(),
            batch_id: batch_id.into(),
            execution_class: ExecutionClass::Cpu,
            handle: TaskHandle {
                task_id: task_id.into(),
                protocol_id: "test.protocol".into(),
                target_binding_id: None,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: None,
                correlation_id: None,
            },
            deadline_tick: None,
            wall_clock_deadline_at: None,
            cancel_requested_at: None,
            worker_id: None,
            worker_started_at: None,
            isolation: RunnerIsolation::Cooperative,
            management: None,
            async_handle: None,
        }
    }

    #[test]
    fn running_batch_count_deduplicates_entries_by_batch_id() {
        let mut running_batches_by_task = BTreeMap::new();
        running_batches_by_task.insert(
            "task-a".into(),
            running_batch("batch.runner", "batch-1", "task-a"),
        );
        running_batches_by_task.insert(
            "task-b".into(),
            running_batch("batch.runner", "batch-1", "task-b"),
        );
        running_batches_by_task.insert(
            "task-c".into(),
            running_batch("batch.runner", "batch-2", "task-c"),
        );
        running_batches_by_task.insert(
            "task-d".into(),
            running_batch("other.runner", "batch-3", "task-d"),
        );

        assert_eq!(
            running_batch_count_for_runner(
                &running_batches_by_task,
                &RunnerId::from("batch.runner")
            ),
            2
        );
    }
}
