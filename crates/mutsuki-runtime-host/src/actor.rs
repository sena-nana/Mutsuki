mod cancellation;
mod lifecycle;
mod mailbox;
mod scheduling;
mod supervision;

use lifecycle::*;
use mailbox::{ActorMailboxes, ActorReceiver, receive_actor_message};
pub(crate) use mailbox::{ActorSender, CoreActorMsg, actor_channel};
use scheduling::*;
use supervision::*;

use futures_channel::oneshot;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::AsyncInvocation;
use mutsuki_runtime_core::{CoreRuntime, RunnerLoopReport, RuntimeResult, TaskRecord};
use mutsuki_runtime_sdk::{HostTaskFailureSummary, HostTaskSnapshot};

use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::host_failure;
use crate::host::{HostRuntimeConfig, TaskCompletionHub};
use crate::management::ManagementExecutor;
use crate::resource_router;
use crate::worker::WorkerPools;

use self::cancellation::{queue_management_retry, request_running_cancel};

/// Owns actor state and orchestrates the focused mailbox, scheduling, supervision,
/// and lifecycle modules without moving runtime facts out of `CoreRuntime`.
struct CoreActor {
    core: CoreRuntime,
    config: HostRuntimeConfig,
    control_rx: ActorReceiver,
    data_rx: ActorReceiver,
    wake_rx: mpsc::Receiver<()>,
    pools: WorkerPools,
    management: ManagementExecutor,
    completion_hub: Arc<TaskCompletionHub>,
    pending_cancels: BTreeMap<String, Vec<String>>,
    running_batches_by_task: BTreeMap<String, RunningBatch>,
    draining_invocations: BTreeMap<String, DrainingInvocation>,
    driver: DriverState,
    terminal_revision: u64,
    task_revision: u64,
    control_burst: usize,
    submitted_at: BTreeMap<String, Instant>,
    pending_task_waits: Vec<PendingTaskWait>,
}

struct ActorExecution {
    pools: WorkerPools,
    management: ManagementExecutor,
}

impl CoreActor {
    #[inline]
    fn new(
        core: CoreRuntime,
        config: HostRuntimeConfig,
        mailboxes: ActorMailboxes,
        execution: ActorExecution,
        completion_hub: Arc<TaskCompletionHub>,
    ) -> Self {
        let terminal_revision = terminal_revision(&core);
        let task_revision = core.tasks().revision();
        Self {
            core,
            config,
            control_rx: mailboxes.control,
            data_rx: mailboxes.data,
            wake_rx: mailboxes.wake,
            pools: execution.pools,
            management: execution.management,
            completion_hub,
            pending_cancels: BTreeMap::new(),
            running_batches_by_task: BTreeMap::new(),
            draining_invocations: BTreeMap::new(),
            driver: DriverState::default(),
            terminal_revision,
            task_revision,
            control_burst: 0,
            submitted_at: BTreeMap::new(),
            pending_task_waits: Vec::new(),
        }
    }

    #[inline(never)]
    fn run(mut self) {
        loop {
            self.refresh_driver();
            let wait = self
                .driver
                .next_wake_deadline(&self.config, &self.running_batches_by_task)
                .into_iter()
                .chain(
                    self.pending_task_waits
                        .iter()
                        .map(|pending| pending.deadline),
                )
                .min()
                .map(|deadline| deadline.saturating_duration_since(Instant::now()));
            let received = receive_actor_message(
                &self.control_rx,
                &self.data_rx,
                &self.wake_rx,
                wait,
                self.config.actor_control_quota,
                &mut self.control_burst,
            );
            match received {
                Ok(message) => {
                    if self.handle_message(message) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) if self.handle_timeout() => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        resolve_pending_task_waits(&self.core, &mut self.pending_task_waits, true);
        self.completion_hub.close();
    }

    #[inline(always)]
    fn refresh_driver(&mut self) {
        self.driver.refresh_scheduled_tick(
            next_required_tick(&self.core, &self.running_batches_by_task),
            self.core.current_step(),
            self.config.tick_interval,
        );
    }

    #[inline(always)]
    fn handle_timeout(&mut self) -> bool {
        self.driver.timed_wakeups = self.driver.timed_wakeups.saturating_add(1);
        if let Some((target_step, deadline)) = self.driver.scheduled_tick
            && deadline <= Instant::now()
            && schedule_ready_at(
                target_step,
                &mut self.core,
                &self.config,
                &mut self.pools,
                &mut self.running_batches_by_task,
            )
            .is_err()
        {
            return true;
        }
        self.supervise();
        self.publish_terminal_changes();
        self.publish_task_changes();
        resolve_pending_task_waits(&self.core, &mut self.pending_task_waits, false);
        false
    }

    #[inline(always)]
    fn handle_message(&mut self, message: CoreActorMsg) -> bool {
        self.supervise();
        let shutdown = match message {
            CoreActorMsg::Command(command, reply_tx) => {
                match &command {
                    HostRuntimeCommand::SubmitTask(task) => {
                        self.submitted_at
                            .insert(task.task_id.clone(), Instant::now());
                    }
                    HostRuntimeCommand::SubmitBatch(batch) => {
                        let submitted = Instant::now();
                        for task in &batch.tasks {
                            self.submitted_at.insert(task.task_id.clone(), submitted);
                        }
                    }
                    _ => {}
                }
                let cancel_started =
                    matches!(&command, HostRuntimeCommand::CancelTask(_)).then(Instant::now);
                let result = handle_command(self, command);
                if let Some(started) = cancel_started {
                    self.config
                        .actor_metrics
                        .record_cancel_propagation(started.elapsed());
                }
                send_command_reply(result, reply_tx)
            }
            CoreActorMsg::AsyncResourceCommand(command, reply_tx) => {
                start_async_resource_command(command, reply_tx, &self.config);
                false
            }
            CoreActorMsg::TaskStatus(task_id, reply_tx) => {
                let _ = reply_tx.send(task_status(&self.core, &task_id));
                false
            }
            CoreActorMsg::WaitTaskStates {
                handles,
                deadline,
                reply,
            } => {
                register_task_wait(
                    &self.core,
                    &mut self.pending_task_waits,
                    handles,
                    deadline,
                    reply,
                );
                false
            }
            CoreActorMsg::WorkerStarted(started) => {
                for task_id in &started.task_ids {
                    if let Some(submitted) = self.submitted_at.remove(task_id) {
                        self.config
                            .actor_metrics
                            .record_submit_to_dispatch(submitted.elapsed());
                    }
                }
                mark_worker_started(started, &mut self.running_batches_by_task);
                false
            }
            CoreActorMsg::WorkerCompleted(completion) => {
                let route_started = Instant::now();
                let _ = handle_worker_completion(
                    completion,
                    &mut self.core,
                    &mut self.pending_cancels,
                    &mut self.running_batches_by_task,
                    &mut self.draining_invocations,
                );
                self.publish_terminal_changes();
                let _ = schedule_ready(
                    &mut self.core,
                    &self.config,
                    &mut self.pools,
                    &mut self.running_batches_by_task,
                );
                self.config
                    .actor_metrics
                    .record_completion_route(route_started.elapsed());
                false
            }
            CoreActorMsg::AsyncEvent(event) => {
                let _ = handle_async_event(
                    event,
                    &mut self.core,
                    &mut self.pending_cancels,
                    &mut self.running_batches_by_task,
                    &mut self.draining_invocations,
                );
                self.publish_terminal_changes();
                let _ = schedule_ready(
                    &mut self.core,
                    &self.config,
                    &mut self.pools,
                    &mut self.running_batches_by_task,
                );
                false
            }
            CoreActorMsg::WorkerExited(exited) => {
                if exited.isolated
                    && let Some(pool) = self.pools.get_mut(&exited.execution_class)
                {
                    let _ = pool.replace_exited_worker(&exited.worker_id);
                }
                false
            }
            CoreActorMsg::ManagementFailed {
                runner_id,
                invocation_id,
            } => {
                queue_management_retry(
                    runner_id,
                    invocation_id,
                    &self.running_batches_by_task,
                    &mut self.pending_cancels,
                );
                false
            }
            CoreActorMsg::Shutdown => {
                if let Some(executor) = &self.config.async_executor {
                    let _ = executor.cancel_all();
                }
                let _ = self.core.abort("host.shutdown");
                true
            }
        };
        self.publish_terminal_changes();
        self.publish_task_changes();
        let waiter_deadline_reached = self
            .pending_task_waits
            .iter()
            .any(|pending| pending.deadline <= Instant::now());
        if waiter_deadline_reached || shutdown {
            resolve_pending_task_waits(&self.core, &mut self.pending_task_waits, shutdown);
        }
        shutdown
    }

    #[inline(always)]
    fn supervise(&mut self) {
        supervise_running_invocations(
            &mut self.core,
            &self.config,
            &mut self.pools,
            &self.management,
            &mut self.pending_cancels,
            &mut self.running_batches_by_task,
            &mut self.draining_invocations,
        );
    }

    #[inline(always)]
    fn publish_terminal_changes(&mut self) {
        publish_terminal_changes(
            &self.core,
            &mut self.terminal_revision,
            &self.completion_hub,
            &mut self.pending_task_waits,
        );
    }

    #[inline(always)]
    fn publish_task_changes(&mut self) {
        let revision = self.core.tasks().revision();
        if revision != self.task_revision {
            self.task_revision = revision;
            self.completion_hub.publish_change();
        }
    }
}

pub(crate) fn core_actor_loop(
    core: CoreRuntime,
    config: HostRuntimeConfig,
    control_rx: ActorReceiver,
    data_rx: ActorReceiver,
    wake_rx: mpsc::Receiver<()>,
    pools: WorkerPools,
    management: ManagementExecutor,
    completion_hub: Arc<TaskCompletionHub>,
) {
    CoreActor::new(
        core,
        config,
        ActorMailboxes {
            control: control_rx,
            data: data_rx,
            wake: wake_rx,
        },
        ActorExecution { pools, management },
        completion_hub,
    )
    .run();
}

#[inline(never)]
fn handle_command(
    actor: &mut CoreActor,
    command: HostRuntimeCommand,
) -> RuntimeResult<(HostRuntimeReply, bool)> {
    let command = match command {
        HostRuntimeCommand::TickOnce => {
            let mut report = schedule_ready(
                &mut actor.core,
                &actor.config,
                &mut actor.pools,
                &mut actor.running_batches_by_task,
            )?;
            let shutdown = drain_worker_completions(actor, &mut report, 1);
            return Ok((HostRuntimeReply::Tick(report), shutdown));
        }
        HostRuntimeCommand::RunUntilIdle { max_ticks } => {
            let mut shutdown = false;
            let mut aggregate = RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: 0,
            };
            for _ in 0..max_ticks {
                let report = schedule_ready(
                    &mut actor.core,
                    &actor.config,
                    &mut actor.pools,
                    &mut actor.running_batches_by_task,
                )?;
                aggregate.claimed_tasks += report.claimed_tasks;
                aggregate.completed_tasks += report.completed_tasks;
                shutdown = drain_worker_completions(actor, &mut aggregate, 8);
                if actor.core.tasks().ready_count() == 0 && actor.core.tasks().running_count() == 0
                {
                    break;
                }
                if shutdown {
                    break;
                }
            }
            return Ok((HostRuntimeReply::Idle(aggregate), shutdown));
        }
        command => command,
    };
    let CoreActor {
        core,
        config,
        pools,
        management,
        data_rx: rx,
        pending_cancels,
        running_batches_by_task,
        draining_invocations,
        driver,
        ..
    } = actor;
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
        HostRuntimeCommand::TickOnce | HostRuntimeCommand::RunUntilIdle { .. } => {
            unreachable!("drive commands are handled before state borrowing")
        }
        HostRuntimeCommand::CancelTask(handle) => {
            let cancellation_targets = core.task_cancellation_targets(&handle);
            let running_invocations = cancellation_targets
                .iter()
                .filter_map(|(task_id, _runner_id)| {
                    running_batches_by_task
                        .get(task_id)
                        .map(|task| task.invocation_id.clone())
                })
                .collect::<BTreeSet<_>>();
            let co_batch_handles = running_batches_by_task
                .values()
                .filter(|task| running_invocations.contains(&task.invocation_id))
                .map(|task| (task.handle.task_id.clone(), task.handle.clone()))
                .collect::<BTreeMap<_, _>>();
            core.cancel_task_handle(&handle)?;
            for co_batch_handle in co_batch_handles.values() {
                if co_batch_handle.task_id != handle.task_id {
                    core.cancel_task_handle(co_batch_handle)?;
                }
            }
            for invocation_id in running_invocations {
                if cancel_async_invocation(&invocation_id, config, running_batches_by_task) {
                    continue;
                }
                request_running_cancel(
                    &invocation_id,
                    management,
                    running_batches_by_task,
                    pending_cancels,
                );
            }
            for (task_id, runner_id) in cancellation_targets {
                if !running_batches_by_task.contains_key(&task_id)
                    && core.cancel_runner_invocation(&runner_id, &task_id).is_err()
                {
                    let pending = pending_cancels.entry(runner_id).or_default();
                    if !pending.contains(&task_id) {
                        pending.push(task_id);
                    }
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
                .map(|task| task.invocation_id.clone())
                .collect();
            for invocation_id in running_invocations {
                if cancel_async_invocation(&invocation_id, config, running_batches_by_task) {
                    continue;
                }
                request_running_cancel(
                    &invocation_id,
                    management,
                    running_batches_by_task,
                    pending_cancels,
                );
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
        HostRuntimeCommand::AsyncExecutor => Ok((
            HostRuntimeReply::AsyncExecutor(
                config
                    .async_executor
                    .as_ref()
                    .map(|executor| executor.snapshot()),
            ),
            false,
        )),
        HostRuntimeCommand::TaskSnapshots => {
            Ok((HostRuntimeReply::TaskSnapshots(task_snapshots(core)), false))
        }
        HostRuntimeCommand::TaskStatesBatch(handles) => Ok((
            HostRuntimeReply::TaskStatesBatch(task_states(core, &handles)?),
            false,
        )),
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
                management,
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
fn send_command_reply(
    outcome: RuntimeResult<(HostRuntimeReply, bool)>,
    reply_tx: mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
) -> bool {
    let shutdown = outcome.as_ref().is_ok_and(|(_, shutdown)| *shutdown);
    let reply = outcome.map(|(reply, _)| reply);
    let _ = reply_tx.send(reply);
    shutdown
}

fn drain_worker_completions(
    actor: &mut CoreActor,
    aggregate: &mut RunnerLoopReport,
    max_messages: usize,
) -> bool {
    for _ in 0..max_messages {
        actor.supervise();
        match actor.data_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(CoreActorMsg::WorkerStarted(started)) => {
                mark_worker_started(started, &mut actor.running_batches_by_task);
            }
            Ok(CoreActorMsg::WorkerCompleted(completion)) => {
                if let Ok(report) = handle_worker_completion(
                    completion,
                    &mut actor.core,
                    &mut actor.pending_cancels,
                    &mut actor.running_batches_by_task,
                    &mut actor.draining_invocations,
                ) {
                    aggregate.completed_tasks += report.completed_tasks;
                }
                if let Ok(report) = schedule_ready(
                    &mut actor.core,
                    &actor.config,
                    &mut actor.pools,
                    &mut actor.running_batches_by_task,
                ) {
                    aggregate.claimed_tasks += report.claimed_tasks;
                    aggregate.completed_tasks += report.completed_tasks;
                }
            }
            Ok(CoreActorMsg::AsyncEvent(event)) => {
                if let Ok(report) = handle_async_event(
                    event,
                    &mut actor.core,
                    &mut actor.pending_cancels,
                    &mut actor.running_batches_by_task,
                    &mut actor.draining_invocations,
                ) {
                    aggregate.completed_tasks += report.completed_tasks;
                }
                if let Ok(report) = schedule_ready(
                    &mut actor.core,
                    &actor.config,
                    &mut actor.pools,
                    &mut actor.running_batches_by_task,
                ) {
                    aggregate.claimed_tasks += report.claimed_tasks;
                    aggregate.completed_tasks += report.completed_tasks;
                }
            }
            Ok(CoreActorMsg::WorkerExited(exited)) => {
                if exited.isolated
                    && let Some(pool) = actor.pools.get_mut(&exited.execution_class)
                {
                    let _ = pool.replace_exited_worker(&exited.worker_id);
                }
            }
            Ok(CoreActorMsg::ManagementFailed {
                runner_id,
                invocation_id,
            }) => queue_management_retry(
                runner_id,
                invocation_id,
                &actor.running_batches_by_task,
                &mut actor.pending_cancels,
            ),
            Ok(CoreActorMsg::TaskStatus(task_id, reply_tx)) => {
                let _ = reply_tx.send(task_status(&actor.core, &task_id));
            }
            Ok(CoreActorMsg::WaitTaskStates { reply, .. }) => {
                let _ = reply.send(Err(host_failure(
                    "host.actor.mailbox",
                    "task waits must use the control mailbox",
                )));
            }
            Ok(CoreActorMsg::Command(command, reply_tx)) => {
                if send_command_reply(handle_command(actor, command), reply_tx) {
                    return true;
                }
            }
            Ok(CoreActorMsg::AsyncResourceCommand(command, reply_tx)) => {
                start_async_resource_command(command, reply_tx, &actor.config);
            }
            Ok(CoreActorMsg::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => return false,
        }
    }
    false
}

fn start_async_resource_command(
    command: HostRuntimeCommand,
    reply: oneshot::Sender<RuntimeResult<HostRuntimeReply>>,
    config: &HostRuntimeConfig,
) {
    let Some(executor) = config.async_executor.as_ref() else {
        let _ = reply.send(Err(host_failure(
            "host.async_executor.unavailable",
            "async resource plan requires an async executor",
        )));
        return;
    };
    let Some(events) = config.async_event_sink.clone() else {
        let _ = reply.send(Err(host_failure(
            "host.async_executor.event_sink",
            "async executor event sink is not configured",
        )));
        return;
    };
    let (provider_id, future, payload_bytes) =
        match resource_router::prepare_async_resource_command(command, config) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let _ = reply.send(Err(failure));
                return;
            }
        };
    let sequence = config
        .async_resource_sequence
        .fetch_add(1, AtomicOrdering::Relaxed)
        .saturating_add(1);
    let invocation_id = format!("async-resource-{sequence}-{provider_id}");
    let deadline_after_ms = config
        .default_runner_limits
        .wall_clock_deadline
        .and_then(|deadline| u64::try_from(deadline.as_millis()).ok());
    let invocation = AsyncInvocation {
        invocation_id: invocation_id.clone(),
        batch_id: invocation_id.clone(),
        runner_id: format!("resource:{provider_id}"),
        task_ids: Vec::new(),
        task_lease_ids: Vec::new(),
        attempt_generations: Vec::new(),
        task_leases: Vec::new(),
        expected_entries: Vec::new(),
        registry_generation: 0,
        plugin_generation: 0,
        cancel_token: invocation_id,
        deadline_after_ms,
        entry_count: 0,
        payload_bytes,
    };
    let _ = executor.spawn_resource(invocation, future, reply, events);
}
