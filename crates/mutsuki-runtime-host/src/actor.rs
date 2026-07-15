use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    CancelPolicy, CompletionBatch, ExecutionClass, TaskHandle, TaskStatus, WorkBatch,
};
use mutsuki_runtime_core::{
    CoreRuntime, ReloadDecision, Runner, RunnerCompletion, RunnerIsolation, RunnerLoopReport,
    RuntimeResult, TaskRecord,
};
use mutsuki_runtime_sdk::{HostTaskFailureSummary, HostTaskSnapshot};

use crate::PreparedRuntimeReload;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply, HostTaskState};
use crate::error::host_failure;
use crate::host::{HostRuntimeConfig, HostRuntimeDriveState, TaskCompletionHub};
use crate::resource_router;
use crate::scheduler::decide_schedule;
use crate::worker::{WorkerDispatchError, WorkerExited, WorkerPools, WorkerStarted};

// Mailbox messages own structured Host commands; boxing would add allocation to every command.
#[allow(clippy::large_enum_variant)]
pub(crate) enum CoreActorMsg {
    Command(
        HostRuntimeCommand,
        mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
    ),
    TaskStatus(String, mpsc::Sender<Option<TaskStatus>>),
    WorkerStarted(WorkerStarted),
    WorkerCompleted(RunnerCompletion),
    WorkerExited(WorkerExited),
    Shutdown,
}

#[derive(Clone, Debug)]
struct RunningBatch {
    runner_id: String,
    invocation_id: String,
    batch_id: String,
    execution_class: ExecutionClass,
    handle: TaskHandle,
    deadline_tick: Option<u64>,
    wall_clock_deadline_at: Option<Instant>,
    cancel_requested_at: Option<Instant>,
    worker_id: Option<String>,
    worker_started_at: Option<Instant>,
    isolation: RunnerIsolation,
}

struct DrainingInvocation {
    runner_id: String,
    recover_after_termination: bool,
}

#[derive(Default)]
struct DriverState {
    scheduled_tick: Option<(u64, Instant)>,
    timed_wakeups: u64,
}

impl DriverState {
    fn refresh_scheduled_tick(
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

    fn next_wake_deadline(
        &self,
        config: &HostRuntimeConfig,
        running_batches_by_task: &BTreeMap<String, RunningBatch>,
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

    fn snapshot(
        &self,
        core: &CoreRuntime,
        config: &HostRuntimeConfig,
        running_batches_by_task: &BTreeMap<String, RunningBatch>,
    ) -> HostRuntimeDriveState {
        HostRuntimeDriveState {
            current_step: core.current_step(),
            next_required_tick: next_required_tick(core, running_batches_by_task),
            next_wake_deadline: self.next_wake_deadline(config, running_batches_by_task),
            timed_wakeups: self.timed_wakeups,
        }
    }
}

fn next_required_tick(
    core: &CoreRuntime,
    running_batches_by_task: &BTreeMap<String, RunningBatch>,
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

pub(crate) fn core_actor_loop(
    mut core: CoreRuntime,
    config: HostRuntimeConfig,
    rx: mpsc::Receiver<CoreActorMsg>,
    mut pools: WorkerPools,
    completion_hub: std::sync::Arc<TaskCompletionHub>,
) {
    let mut pending_cancels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut running_batches_by_task: BTreeMap<String, RunningBatch> = BTreeMap::new();
    let mut draining_invocations: BTreeMap<String, DrainingInvocation> = BTreeMap::new();
    let mut driver = DriverState::default();
    let mut terminal_revision = terminal_revision(&core);
    loop {
        driver.refresh_scheduled_tick(
            next_required_tick(&core, &running_batches_by_task),
            core.current_step(),
            config.tick_interval,
        );
        let wait = driver
            .next_wake_deadline(&config, &running_batches_by_task)
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let received = match wait {
            Some(wait) => rx.recv_timeout(wait),
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        let msg = match received {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                driver.timed_wakeups = driver.timed_wakeups.saturating_add(1);
                if let Some((target_step, deadline)) = driver.scheduled_tick
                    && deadline <= Instant::now()
                    && schedule_ready_at(
                        target_step,
                        &mut core,
                        &config,
                        &mut pools,
                        &mut running_batches_by_task,
                    )
                    .is_err()
                {
                    break;
                }
                supervise_running_invocations(
                    &mut core,
                    &config,
                    &mut pools,
                    &mut pending_cancels,
                    &mut running_batches_by_task,
                    &mut draining_invocations,
                );
                publish_terminal_changes(&core, &mut terminal_revision, &completion_hub);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        supervise_running_invocations(
            &mut core,
            &config,
            &mut pools,
            &mut pending_cancels,
            &mut running_batches_by_task,
            &mut draining_invocations,
        );
        let shutdown = match msg {
            CoreActorMsg::Command(command, reply_tx) => send_command_reply(
                handle_command(
                    command,
                    &mut core,
                    &config,
                    &mut pools,
                    &rx,
                    &mut pending_cancels,
                    &mut running_batches_by_task,
                    &mut draining_invocations,
                    &driver,
                ),
                reply_tx,
            ),
            CoreActorMsg::TaskStatus(task_id, reply_tx) => {
                let _ = reply_tx.send(task_status(&core, &task_id));
                false
            }
            CoreActorMsg::WorkerStarted(started) => {
                mark_worker_started(started, &mut running_batches_by_task);
                false
            }
            CoreActorMsg::WorkerCompleted(completion) => {
                let _ = handle_worker_completion(
                    completion,
                    &mut core,
                    &mut pending_cancels,
                    &mut running_batches_by_task,
                    &mut draining_invocations,
                );
                let _ =
                    schedule_ready(&mut core, &config, &mut pools, &mut running_batches_by_task);
                false
            }
            CoreActorMsg::WorkerExited(exited) => {
                if exited.isolated
                    && let Some(pool) = pools.get_mut(&exited.execution_class)
                {
                    let _ = pool.replace_exited_worker(&exited.worker_id);
                }
                false
            }
            CoreActorMsg::Shutdown => {
                let _ = core.abort("host.shutdown");
                true
            }
        };
        publish_terminal_changes(&core, &mut terminal_revision, &completion_hub);
        if shutdown {
            break;
        }
    }
    completion_hub.close();
}

fn terminal_revision(core: &CoreRuntime) -> u64 {
    let statistics = core.tasks().statistics();
    (statistics.completed
        + statistics.failed
        + statistics.cancelled
        + statistics.expired
        + statistics.dead_letter) as u64
        + statistics.terminal_records_evicted
}

fn publish_terminal_changes(
    core: &CoreRuntime,
    previous_revision: &mut u64,
    completion_hub: &TaskCompletionHub,
) {
    let revision = terminal_revision(core);
    if revision > *previous_revision {
        *previous_revision = revision;
        completion_hub.publish(revision);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    command: HostRuntimeCommand,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
    driver: &DriverState,
) -> RuntimeResult<(HostRuntimeReply, bool)> {
    match command {
        HostRuntimeCommand::SubmitTask(task) => {
            let handle = core.submit_task(*task)?;
            if config.event_driven {
                schedule_ready(core, config, pools, running_batches_by_task)?;
            }
            Ok((HostRuntimeReply::TaskSubmitted(handle), false))
        }
        HostRuntimeCommand::SubmitBatch(batch) => {
            let handles = core.submit_batch(*batch)?;
            if config.event_driven {
                schedule_ready(core, config, pools, running_batches_by_task)?;
            }
            Ok((HostRuntimeReply::TaskBatchSubmitted(handles), false))
        }
        HostRuntimeCommand::TickOnce => {
            let mut report = schedule_ready(core, config, pools, running_batches_by_task)?;
            let shutdown = drain_worker_completions(
                core,
                config,
                pools,
                rx,
                pending_cancels,
                running_batches_by_task,
                draining_invocations,
                driver,
                &mut report,
                1,
            );
            Ok((HostRuntimeReply::Tick(report), shutdown))
        }
        HostRuntimeCommand::RunUntilIdle { max_ticks } => {
            let mut shutdown = false;
            let mut aggregate = RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: 0,
            };
            for _ in 0..max_ticks {
                let report = schedule_ready(core, config, pools, running_batches_by_task)?;
                aggregate.claimed_tasks += report.claimed_tasks;
                aggregate.completed_tasks += report.completed_tasks;
                shutdown = drain_worker_completions(
                    core,
                    config,
                    pools,
                    rx,
                    pending_cancels,
                    running_batches_by_task,
                    draining_invocations,
                    driver,
                    &mut aggregate,
                    8,
                );
                if core.tasks().ready_count() == 0 && core.tasks().running_count() == 0 {
                    break;
                }
                if shutdown {
                    break;
                }
            }
            Ok((HostRuntimeReply::Idle(aggregate), shutdown))
        }
        HostRuntimeCommand::CancelTask(handle) => {
            let cancellation_targets = core.task_cancellation_targets(&handle);
            core.cancel_task_handle(&handle)?;
            for (task_id, runner_id) in cancellation_targets {
                let running_invocation = running_batches_by_task
                    .get(&task_id)
                    .map(|task| task.invocation_id.clone());
                if core.cancel_runner_invocation(&runner_id, &task_id).is_ok() {
                    continue;
                }
                let pending = pending_cancels.entry(runner_id).or_default();
                if let Some(invocation_id) = running_invocation {
                    mark_cancel_requested(&invocation_id, running_batches_by_task);
                    if !pending.contains(&invocation_id) {
                        pending.push(invocation_id);
                    }
                } else if !pending.contains(&task_id) {
                    pending.push(task_id);
                }
            }
            Ok((HostRuntimeReply::TaskCancelled(handle), false))
        }
        HostRuntimeCommand::BeginDrain => {
            Ok((HostRuntimeReply::DrainStarted(core.begin_drain()?), false))
        }
        HostRuntimeCommand::Abort { reason } => {
            let running_invocations: BTreeSet<_> = running_batches_by_task
                .values()
                .map(|task| (task.runner_id.clone(), task.invocation_id.clone()))
                .collect();
            for (runner_id, invocation_id) in running_invocations {
                mark_cancel_requested(&invocation_id, running_batches_by_task);
                pending_cancels
                    .entry(runner_id)
                    .or_default()
                    .push(invocation_id);
            }
            let cancelled_tasks = core.abort(reason)?;
            Ok((HostRuntimeReply::RuntimeAborted { cancelled_tasks }, false))
        }
        HostRuntimeCommand::StopState => {
            Ok((HostRuntimeReply::StopState(core.stop_state()), false))
        }
        HostRuntimeCommand::Statistics => {
            Ok((HostRuntimeReply::Statistics(core.statistics()), false))
        }
        HostRuntimeCommand::DriveState => Ok((
            HostRuntimeReply::DriveState(driver.snapshot(core, config, running_batches_by_task)),
            false,
        )),
        HostRuntimeCommand::WorkerPools => {
            Ok((HostRuntimeReply::WorkerPools(pools.snapshots()), false))
        }
        HostRuntimeCommand::TaskSnapshots => {
            Ok((HostRuntimeReply::TaskSnapshots(task_snapshots(core)), false))
        }
        HostRuntimeCommand::TaskStatesBatch(handles) => {
            let mut states = Vec::with_capacity(handles.len());
            for handle in handles {
                states.push(HostTaskState {
                    status: core.task_handle_status(&handle),
                    outcome: core.task_handle_outcome(&handle)?,
                    handle,
                });
            }
            Ok((HostRuntimeReply::TaskStatesBatch(states), false))
        }
        HostRuntimeCommand::TaskOutcome(handle) => Ok((
            HostRuntimeReply::TaskOutcome(core.task_handle_outcome(&handle)?),
            false,
        )),
        HostRuntimeCommand::EventsAfter { sequence, limit } => Ok((
            HostRuntimeReply::Events(core.events_after(sequence, limit)),
            false,
        )),
        HostRuntimeCommand::TraceSpansAfter { sequence, limit } => Ok((
            HostRuntimeReply::TraceSpans(core.trace_spans_after(sequence, limit)),
            false,
        )),
        HostRuntimeCommand::OpenResourceDescriptor(ref_id) => Ok((
            HostRuntimeReply::ResourceDescriptor(core.open_resource(&ref_id)?),
            false,
        )),
        HostRuntimeCommand::Reload {
            prepared,
            drain_timeout,
        } => {
            let decision = reload_runtime(
                prepared,
                drain_timeout,
                core,
                config,
                pools,
                rx,
                pending_cancels,
                running_batches_by_task,
                draining_invocations,
            )?;
            if config.event_driven {
                schedule_ready(core, config, pools, running_batches_by_task)?;
            }
            Ok((HostRuntimeReply::Reloaded(decision), false))
        }
        command @ (HostRuntimeCommand::CreateBlobResource { .. }
        | HostRuntimeCommand::CreateCowStateResource { .. }
        | HostRuntimeCommand::CreateCapabilityResource { .. }
        | HostRuntimeCommand::CollectReadPlan(_)
        | HostRuntimeCommand::SnapshotReadPlan { .. }
        | HostRuntimeCommand::OpenStreamPlan(_)
        | HostRuntimeCommand::ExecuteExportPlan(_)
        | HostRuntimeCommand::CommitWritePlan { .. }
        | HostRuntimeCommand::ExecuteCommandPlan(_)
        | HostRuntimeCommand::ExecuteCommandBatch(_)
        | HostRuntimeCommand::ExecuteSagaPlan(_)) => Ok((
            resource_router::handle_resource_command(command, core, config)?,
            false,
        )),
    }
}

fn task_snapshots(core: &CoreRuntime) -> Vec<HostTaskSnapshot> {
    core.tasks()
        .records()
        .into_iter()
        .map(task_snapshot)
        .collect()
}

fn task_snapshot(record: &TaskRecord) -> HostTaskSnapshot {
    HostTaskSnapshot {
        task_id: record.task.task_id.clone(),
        protocol_id: record.task.protocol_id.clone(),
        status: record.status.clone(),
        priority: record.task.priority,
        ready_at_step: record.task.ready_at_step,
        created_sequence: record.task.created_sequence,
        registry_generation: record.task.registry_generation,
        target_binding_id: record.task.target_binding_id.clone(),
        runner_hint: record.task.runner_hint.clone(),
        claimed_by: record.claimed_by.clone(),
        owner_runner: record.owner_runner.clone(),
        lease_id: record.task.lease_id.clone(),
        attempt_generation: record.attempt_generation,
        trace_id: record.task.trace_id.clone(),
        correlation_id: record.task.correlation_id.clone(),
        input_refs: record.task.input_refs.clone(),
        output_ref: record.task.output_ref.clone(),
        continuation_ref: record.task.continuation_ref.clone(),
        required_surfaces: record.task.required_surfaces.clone(),
        failure: record
            .failure
            .as_ref()
            .map(|failure| HostTaskFailureSummary {
                code: failure.code.clone(),
                source: failure.source.clone(),
                route: failure.route.clone(),
            }),
    }
}

#[allow(clippy::too_many_arguments)]
fn reload_runtime(
    prepared: PreparedRuntimeReload,
    drain_timeout: Duration,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
) -> RuntimeResult<ReloadDecision> {
    drain_for_reload(
        core,
        config,
        pools,
        rx,
        pending_cancels,
        running_batches_by_task,
        draining_invocations,
        drain_timeout,
    )?;
    let runners = prepared
        .runners
        .into_iter()
        .map(|runner| Box::new(DisposeOnDropRunner::new(runner)) as Box<dyn Runner>)
        .collect();
    core.reload_with_runners(prepared.plan, runners)
}

#[allow(clippy::too_many_arguments)]
fn drain_for_reload(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
    drain_timeout: Duration,
) -> RuntimeResult<()> {
    let started_at = Instant::now();
    loop {
        supervise_running_invocations(
            core,
            config,
            pools,
            pending_cancels,
            running_batches_by_task,
            draining_invocations,
        );
        if running_batches_by_task.is_empty() {
            return Ok(());
        }
        let elapsed = started_at.elapsed();
        if elapsed >= drain_timeout {
            return Err(host_failure(
                "host.reload.drain_timeout",
                format!(
                    "timed out waiting for {} running batch entry/entries to drain",
                    running_batches_by_task.len()
                ),
            ));
        }
        let wait = drain_timeout
            .saturating_sub(elapsed)
            .min(Duration::from_millis(10));
        match rx.recv_timeout(wait) {
            Ok(CoreActorMsg::WorkerStarted(started)) => {
                mark_worker_started(started, running_batches_by_task);
            }
            Ok(CoreActorMsg::WorkerCompleted(completion)) => {
                let _ = handle_worker_completion(
                    completion,
                    core,
                    pending_cancels,
                    running_batches_by_task,
                    draining_invocations,
                )?;
            }
            Ok(CoreActorMsg::WorkerExited(exited)) => {
                if exited.isolated
                    && let Some(pool) = pools.get_mut(&exited.execution_class)
                {
                    pool.replace_exited_worker(&exited.worker_id)?;
                }
            }
            Ok(CoreActorMsg::TaskStatus(task_id, reply_tx)) => {
                let _ = reply_tx.send(task_status(core, &task_id));
            }
            Ok(CoreActorMsg::Command(_, reply_tx)) => {
                let _ = reply_tx.send(Err(host_failure(
                    "host.reload.busy",
                    "runtime reload is draining active work",
                )));
            }
            Ok(CoreActorMsg::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(host_failure(
                    "host.reload.shutdown",
                    "runtime actor stopped",
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

struct DisposeOnDropRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    inner: Box<dyn Runner>,
    disposed: bool,
}

impl DisposeOnDropRunner {
    fn new(inner: Box<dyn Runner>) -> Self {
        Self {
            descriptor: inner.descriptor().clone(),
            inner,
            disposed: false,
        }
    }
}

impl Runner for DisposeOnDropRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: mutsuki_runtime_contracts::RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.inner.run_batch(ctx, batch)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.inner.cancel(invocation_id)
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.disposed = true;
        self.inner.dispose()
    }

    fn isolation(&self) -> RunnerIsolation {
        self.inner.isolation()
    }

    fn recover_after_hard_termination(&mut self) -> RuntimeResult<()> {
        self.inner.recover_after_hard_termination()
    }
}

impl Drop for DisposeOnDropRunner {
    fn drop(&mut self) {
        if !self.disposed {
            let _ = self.inner.dispose();
            self.disposed = true;
        }
    }
}

fn send_command_reply(
    outcome: RuntimeResult<(HostRuntimeReply, bool)>,
    reply_tx: mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
) -> bool {
    let shutdown = outcome.as_ref().is_ok_and(|(_, shutdown)| *shutdown);
    let reply = outcome.map(|(reply, _)| reply);
    let _ = reply_tx.send(reply);
    shutdown
}

#[allow(clippy::too_many_arguments)]
fn drain_worker_completions(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
    driver: &DriverState,
    aggregate: &mut RunnerLoopReport,
    max_messages: usize,
) -> bool {
    for _ in 0..max_messages {
        supervise_running_invocations(
            core,
            config,
            pools,
            pending_cancels,
            running_batches_by_task,
            draining_invocations,
        );
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(CoreActorMsg::WorkerStarted(started)) => {
                mark_worker_started(started, running_batches_by_task);
            }
            Ok(CoreActorMsg::WorkerCompleted(completion)) => {
                if let Ok(report) = handle_worker_completion(
                    completion,
                    core,
                    pending_cancels,
                    running_batches_by_task,
                    draining_invocations,
                ) {
                    aggregate.completed_tasks += report.completed_tasks;
                }
                if let Ok(report) = schedule_ready(core, config, pools, running_batches_by_task) {
                    aggregate.claimed_tasks += report.claimed_tasks;
                    aggregate.completed_tasks += report.completed_tasks;
                }
            }
            Ok(CoreActorMsg::WorkerExited(exited)) => {
                if exited.isolated
                    && let Some(pool) = pools.get_mut(&exited.execution_class)
                {
                    let _ = pool.replace_exited_worker(&exited.worker_id);
                }
            }
            Ok(CoreActorMsg::TaskStatus(task_id, reply_tx)) => {
                let _ = reply_tx.send(task_status(core, &task_id));
            }
            Ok(CoreActorMsg::Command(command, reply_tx)) => {
                if send_command_reply(
                    handle_command(
                        command,
                        core,
                        config,
                        pools,
                        rx,
                        pending_cancels,
                        running_batches_by_task,
                        draining_invocations,
                        driver,
                    ),
                    reply_tx,
                ) {
                    return true;
                }
            }
            Ok(CoreActorMsg::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => return false,
        }
    }
    false
}

fn apply_pending_cancels(
    completion: &mut RunnerCompletion,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) {
    let runner_id = completion.runner.descriptor().runner_id.clone();
    let Some(invocation_ids) = pending_cancels.remove(&runner_id) else {
        return;
    };
    for invocation_id in invocation_ids {
        let _ = completion.runner.cancel(&invocation_id);
    }
}

fn remove_pending_cancel(
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    runner_id: &str,
    invocation_id: &str,
) {
    let remove_runner = if let Some(invocation_ids) = pending_cancels.get_mut(runner_id) {
        invocation_ids.retain(|item| item != invocation_id);
        invocation_ids.is_empty()
    } else {
        false
    };
    if remove_runner {
        pending_cancels.remove(runner_id);
    }
}

fn handle_worker_completion(
    mut completion: RunnerCompletion,
    core: &mut CoreRuntime,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
) -> RuntimeResult<RunnerLoopReport> {
    let invocation_id = completion.batch_id.clone();
    if let Some(draining) = draining_invocations.remove(&invocation_id) {
        remove_pending_cancel(pending_cancels, &draining.runner_id, &invocation_id);
        if draining.recover_after_termination {
            completion.runner.recover_after_hard_termination()?;
            completion.result = Err(host_failure(
                "host.runner.hard_timeout",
                format!("runner {} was terminated and recovered", draining.runner_id),
            ));
            return core.complete_runner_dispatch(completion);
        }
        let _ = completion.runner.cancel(&invocation_id);
        let _ = completion.runner.dispose();
        return Ok(RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: 0,
        });
    }

    apply_pending_cancels(&mut completion, pending_cancels);
    remove_running_batch_entries(&completion, running_batches_by_task);
    core.complete_runner_dispatch(completion)
}

fn supervise_running_invocations(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
) {
    cancel_expired_tick_deadlines(core, pending_cancels, running_batches_by_task);
    let now = Instant::now();
    let expired: Vec<_> = running_batches_by_task
        .values()
        .filter(|task| {
            task.wall_clock_deadline_at
                .is_some_and(|deadline| now >= deadline)
                || task.cancel_requested_at.is_some_and(|cancelled_at| {
                    config
                        .cancel_grace_period
                        .is_some_and(|grace| now.duration_since(cancelled_at) >= grace)
                })
                || task.worker_started_at.is_some_and(|started_at| {
                    config
                        .worker_health_timeout
                        .is_some_and(|timeout| now.duration_since(started_at) >= timeout)
                })
        })
        .map(|task| task.invocation_id.clone())
        .collect();
    for invocation_id in expired {
        isolate_invocation(
            &invocation_id,
            core,
            pools,
            pending_cancels,
            running_batches_by_task,
            draining_invocations,
        );
    }
}

fn cancel_expired_tick_deadlines(
    core: &mut CoreRuntime,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) {
    let current_step = core.current_step();
    let expired: Vec<_> = running_batches_by_task
        .iter()
        .filter(|&(_task_id, task)| {
            task.deadline_tick
                .is_some_and(|deadline_tick| current_step >= deadline_tick)
        })
        .map(|(task_id, _task)| task_id.clone())
        .collect();
    for task_id in expired {
        let Some(task) = running_batches_by_task.remove(&task_id) else {
            continue;
        };
        if task_status(core, &task_id) == Some(TaskStatus::Running) {
            let _ = core.cancel_task_handle(&task.handle);
            pending_cancels
                .entry(task.runner_id)
                .or_default()
                .push(task.invocation_id);
        }
    }
}

fn isolate_invocation(
    invocation_id: &str,
    core: &mut CoreRuntime,
    pools: &mut WorkerPools,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
) {
    if draining_invocations.contains_key(invocation_id) {
        return;
    }
    let task_ids: Vec<_> = running_batches_by_task
        .iter()
        .filter(|&(_task_id, task)| task.invocation_id == invocation_id)
        .map(|(task_id, _task)| task_id.clone())
        .collect();
    let Some(first_task) = task_ids
        .first()
        .and_then(|task_id| running_batches_by_task.get(task_id))
        .cloned()
    else {
        return;
    };
    for task_id in &task_ids {
        if let Some(task) = running_batches_by_task.get(task_id)
            && task_status(core, task_id) == Some(TaskStatus::Running)
        {
            let _ = core.cancel_task_handle(&task.handle);
        }
        running_batches_by_task.remove(task_id);
    }
    pending_cancels
        .entry(first_task.runner_id.clone())
        .or_default()
        .push(invocation_id.to_string());
    let recover_after_termination = match &first_task.isolation {
        RunnerIsolation::Cooperative => false,
        RunnerIsolation::HardProcess(handle) => handle.terminate().is_ok(),
    };
    draining_invocations.insert(
        invocation_id.to_string(),
        DrainingInvocation {
            runner_id: first_task.runner_id.clone(),
            recover_after_termination,
        },
    );
    if let Some(worker_id) = &first_task.worker_id
        && let Some(pool) = pools.get(&first_task.execution_class)
    {
        let _ = pool.isolate(worker_id);
    }
}

fn task_status(core: &CoreRuntime, task_id: &str) -> Option<TaskStatus> {
    core.tasks()
        .get(task_id)
        .map(|record| record.status.clone())
}

fn mark_worker_started(
    started: WorkerStarted,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) {
    let now = Instant::now();
    for task_id in &started.task_ids {
        let Some(task) = running_batches_by_task.get_mut(task_id) else {
            continue;
        };
        if task.invocation_id == started.invocation_id
            && task.runner_id == started.runner_id
            && task.batch_id == started.batch_id
            && task.execution_class == started.execution_class
        {
            task.worker_id = Some(started.worker_id.clone());
            task.worker_started_at = Some(now);
        }
    }
}

fn mark_cancel_requested(
    invocation_id: &str,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) {
    let now = Instant::now();
    for task in running_batches_by_task.values_mut() {
        if task.invocation_id == invocation_id {
            task.cancel_requested_at = Some(now);
        }
    }
}
fn remove_running_batch_entries(
    completion: &RunnerCompletion,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) {
    for lease in &completion.task_leases {
        running_batches_by_task.remove(&lease.task_id);
    }
}

fn running_batch_count_for_runner(
    running_batches_by_task: &BTreeMap<String, RunningBatch>,
    runner_id: &str,
) -> usize {
    running_batches_by_task
        .values()
        .filter(|task| task.runner_id == runner_id)
        .map(|task| task.batch_id.clone())
        .collect::<BTreeSet<_>>()
        .len()
}

fn schedule_ready(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) -> RuntimeResult<RunnerLoopReport> {
    let target_step = core.current_step().saturating_add(1);
    schedule_ready_at(target_step, core, config, pools, running_batches_by_task)
}

fn schedule_ready_at(
    target_step: u64,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) -> RuntimeResult<RunnerLoopReport> {
    let mut compute_reservations = 0usize;
    let mut blocking_reservations = 0usize;
    let (report, dispatches) = core.claim_ready_dispatches_at_step(
        target_step,
        |descriptor, load, current_step, registry_generation| {
            let limits = config
                .runner_limits
                .get(&descriptor.runner_id)
                .unwrap_or(&config.default_runner_limits);
            let reservations = match descriptor.execution_class {
                ExecutionClass::Orchestration | ExecutionClass::Io | ExecutionClass::Cpu => {
                    &mut compute_reservations
                }
                ExecutionClass::Blocking | ExecutionClass::Script => &mut blocking_reservations,
                ExecutionClass::Control => {
                    return Ok(mutsuki_runtime_core::ScheduleDecision::new(
                        "host.default",
                        0,
                        "control.inline",
                    ));
                }
            };
            let (pool_slots, mut pool_capacity) = pools
                .get(&descriptor.execution_class)
                .map(|pool| (pool.available_slots(), pool.capacity()))
                .unwrap_or_default();
            let pool_slots = pool_slots.saturating_sub(*reservations);
            pool_capacity.queued_batches =
                pool_capacity.queued_batches.saturating_add(*reservations);
            let running_batches =
                running_batch_count_for_runner(running_batches_by_task, &descriptor.runner_id);
            let decision = decide_schedule(
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
            if decision.dispatch_limit > 0 && decision.budget.max_batches > 0 {
                *reservations = (*reservations).saturating_add(1);
            }
            Ok(decision)
        },
        None,
    )?;
    let mut deferred_entries = 0usize;
    let mut rejected_entries = 0usize;
    for mut dispatch in dispatches {
        let execution_class = dispatch.runner.descriptor().execution_class.clone();
        let runner_id = dispatch.runner.descriptor().runner_id.clone();
        let limits = config
            .runner_limits
            .get(&runner_id)
            .unwrap_or(&config.default_runner_limits);
        dispatch.ctx.deadline_tick = limits
            .deadline_ticks
            .map(|ticks| dispatch.ctx.current_step.saturating_add(ticks));
        let invocation_id = dispatch.ctx.invocation_id.clone();
        let batch_id = dispatch.ctx.batch_id.clone();
        let isolation = dispatch.runner.isolation();
        let deadline_tick = dispatch.ctx.deadline_tick;
        let wall_clock_deadline_at = limits
            .wall_clock_deadline
            .map(|deadline| Instant::now() + deadline);
        let tasks = dispatch
            .batch
            .row_payload_tasks()
            .map_err(mutsuki_runtime_core::RuntimeFailure::new)?;
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
                        runner: dispatch.runner,
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
            let handle = TaskHandle {
                task_id: task.task_id.clone(),
                protocol_id: task.protocol_id.clone(),
                target_binding_id: task.target_binding_id.clone(),
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id.clone(),
                correlation_id: task.correlation_id.clone(),
            };
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
                },
            );
        }
    }
    Ok(RunnerLoopReport {
        claimed_tasks: report.claimed_tasks.saturating_sub(deferred_entries),
        completed_tasks: report.completed_tasks.saturating_add(rejected_entries),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            running_batch_count_for_runner(&running_batches_by_task, "batch.runner"),
            2
        );
    }
}
