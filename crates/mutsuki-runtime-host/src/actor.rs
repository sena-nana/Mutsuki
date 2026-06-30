use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{ExecutionClass, TaskStatus};
use mutsuki_runtime_core::{CoreRuntime, RunnerCompletion, RunnerLoopReport, RuntimeResult};

use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::host_failure;
use crate::host::HostRuntimeConfig;
use crate::scheduler::decide_schedule;
use crate::worker::{WorkerPool, WorkerStarted, worker_pools};

pub(crate) enum CoreActorMsg {
    Command(
        HostRuntimeCommand,
        mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
    ),
    TaskStatus(String, mpsc::Sender<Option<TaskStatus>>),
    WorkerStarted(WorkerStarted),
    WorkerCompleted(RunnerCompletion),
    Shutdown,
}

#[derive(Clone, Debug)]
struct RunningTask {
    runner_id: String,
    invocation_id: String,
    execution_class: ExecutionClass,
    deadline_tick: Option<u64>,
    wall_clock_deadline_at: Option<Instant>,
    cancel_requested_at: Option<Instant>,
    worker_id: Option<String>,
    worker_started_at: Option<Instant>,
}

pub(crate) fn core_actor_loop(
    mut core: CoreRuntime,
    config: HostRuntimeConfig,
    rx: mpsc::Receiver<CoreActorMsg>,
    actor_tx: mpsc::Sender<CoreActorMsg>,
) {
    let mut pools = match worker_pools(&config, actor_tx) {
        Ok(pools) => pools,
        Err(_) => return,
    };
    let mut pending_cancels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut running_tasks: BTreeMap<String, RunningTask> = BTreeMap::new();
    let mut draining_invocations: BTreeMap<String, String> = BTreeMap::new();
    while let Ok(msg) = rx.recv() {
        supervise_running_invocations(
            &mut core,
            &config,
            &mut pools,
            &mut pending_cancels,
            &mut running_tasks,
            &mut draining_invocations,
        );
        match msg {
            CoreActorMsg::Command(command, reply_tx) => {
                let shutdown = send_command_reply(
                    handle_command(
                        command,
                        &mut core,
                        &config,
                        &mut pools,
                        &rx,
                        &mut pending_cancels,
                        &mut running_tasks,
                        &mut draining_invocations,
                    ),
                    reply_tx,
                );
                if shutdown {
                    break;
                }
            }
            CoreActorMsg::TaskStatus(task_id, reply_tx) => {
                let _ = reply_tx.send(core.task_status(&task_id));
            }
            CoreActorMsg::WorkerStarted(started) => {
                mark_worker_started(started, &mut running_tasks);
            }
            CoreActorMsg::WorkerCompleted(completion) => {
                let _ = handle_worker_completion(
                    completion,
                    &mut core,
                    &mut pending_cancels,
                    &mut running_tasks,
                    &mut draining_invocations,
                );
                let _ = schedule_ready(&mut core, &config, &mut pools, &mut running_tasks);
            }
            CoreActorMsg::Shutdown => break,
        }
    }
}

fn handle_command(
    command: HostRuntimeCommand,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_tasks: &mut BTreeMap<String, RunningTask>,
    draining_invocations: &mut BTreeMap<String, String>,
) -> RuntimeResult<(HostRuntimeReply, bool)> {
    match command {
        HostRuntimeCommand::SubmitTask(task) => {
            let task_id = core.submit_task(*task)?;
            Ok((HostRuntimeReply::TaskSubmitted(task_id), false))
        }
        HostRuntimeCommand::TickOnce => {
            let mut report = schedule_ready(core, config, pools, running_tasks)?;
            let shutdown = drain_worker_completions(
                core,
                config,
                pools,
                rx,
                pending_cancels,
                running_tasks,
                draining_invocations,
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
                let report = schedule_ready(core, config, pools, running_tasks)?;
                aggregate.claimed_tasks += report.claimed_tasks;
                aggregate.completed_tasks += report.completed_tasks;
                shutdown = drain_worker_completions(
                    core,
                    config,
                    pools,
                    rx,
                    pending_cancels,
                    running_tasks,
                    draining_invocations,
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
        HostRuntimeCommand::CancelTask(task_id) => {
            let running_task = running_tasks.get(&task_id).cloned();
            core.cancel_task(&task_id)?;
            if let Some(task) = running_task {
                mark_cancel_requested(&task.invocation_id, running_tasks);
                pending_cancels
                    .entry(task.runner_id)
                    .or_default()
                    .push(task.invocation_id);
            }
            Ok((HostRuntimeReply::TaskCancelled(task_id), false))
        }
        HostRuntimeCommand::TaskOutcome(task_id) => Ok((
            HostRuntimeReply::TaskOutcome(core.task_outcome(&task_id)?),
            false,
        )),
        HostRuntimeCommand::CreateBlobResource { schema, bytes } => Ok((
            HostRuntimeReply::ResourceCreated(core.create_blob_resource(&schema, bytes)?),
            false,
        )),
        HostRuntimeCommand::CreateCowStateResource {
            kind_id,
            schema,
            bytes,
        } => Ok((
            HostRuntimeReply::ResourceCreated(
                core.create_cow_state_resource(&kind_id, &schema, bytes)?,
            ),
            false,
        )),
        HostRuntimeCommand::CreateCapabilityResource { kind_id, schema } => Ok((
            HostRuntimeReply::ResourceCreated(core.create_capability_resource(&kind_id, &schema)?),
            false,
        )),
        HostRuntimeCommand::CollectReadPlan(plan) => Ok((
            HostRuntimeReply::ResourceBytes(core.collect_read_plan(&plan)?),
            false,
        )),
        HostRuntimeCommand::SnapshotReadPlan {
            plan,
            kind_id,
            schema,
        } => Ok((
            HostRuntimeReply::Snapshot(core.snapshot_read_plan(&plan, &kind_id, &schema)?),
            false,
        )),
        HostRuntimeCommand::OpenStreamPlan(plan) => Ok((
            HostRuntimeReply::StreamPlan(core.open_stream_plan(&plan)?),
            false,
        )),
        HostRuntimeCommand::ExecuteExportPlan(plan) => Ok((
            HostRuntimeReply::PlanReceipt(core.execute_export_plan(&plan)?),
            false,
        )),
        HostRuntimeCommand::CommitWritePlan { plan, bytes } => Ok((
            HostRuntimeReply::PlanReceipt(core.commit_write_plan(&plan, bytes)?),
            false,
        )),
        HostRuntimeCommand::ExecuteCommandPlan(plan) => Ok((
            HostRuntimeReply::PlanReceipt(core.execute_command_plan(&plan)?),
            false,
        )),
        HostRuntimeCommand::ExecuteCommandBatch(batch) => Ok((
            HostRuntimeReply::PlanReceipts(core.execute_command_batch(&batch)?),
            false,
        )),
        HostRuntimeCommand::ExecuteSagaPlan(saga) => Ok((
            HostRuntimeReply::PlanReceipts(core.execute_saga_plan(&saga)?),
            false,
        )),
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

fn drain_worker_completions(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_tasks: &mut BTreeMap<String, RunningTask>,
    draining_invocations: &mut BTreeMap<String, String>,
    aggregate: &mut RunnerLoopReport,
    max_messages: usize,
) -> bool {
    for _ in 0..max_messages {
        supervise_running_invocations(
            core,
            config,
            pools,
            pending_cancels,
            running_tasks,
            draining_invocations,
        );
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(CoreActorMsg::WorkerStarted(started)) => {
                mark_worker_started(started, running_tasks);
            }
            Ok(CoreActorMsg::WorkerCompleted(completion)) => {
                if let Ok(report) = handle_worker_completion(
                    completion,
                    core,
                    pending_cancels,
                    running_tasks,
                    draining_invocations,
                ) {
                    aggregate.completed_tasks += report.completed_tasks;
                }
                if let Ok(report) = schedule_ready(core, config, pools, running_tasks) {
                    aggregate.claimed_tasks += report.claimed_tasks;
                    aggregate.completed_tasks += report.completed_tasks;
                }
            }
            Ok(CoreActorMsg::TaskStatus(task_id, reply_tx)) => {
                let _ = reply_tx.send(core.task_status(&task_id));
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
                        running_tasks,
                        draining_invocations,
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
    running_tasks: &mut BTreeMap<String, RunningTask>,
    draining_invocations: &mut BTreeMap<String, String>,
) -> RuntimeResult<RunnerLoopReport> {
    let invocation_id = completion_invocation_id(&completion).unwrap_or_default();
    if let Some(runner_id) = draining_invocations.remove(&invocation_id) {
        remove_pending_cancel(pending_cancels, &runner_id, &invocation_id);
        let _ = completion.runner.cancel(&invocation_id);
        let _ = completion.runner.dispose();
        return Ok(RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: 0,
        });
    }

    apply_pending_cancels(&mut completion, pending_cancels);
    remove_running_tasks(&completion, running_tasks);
    core.complete_runner_dispatch(completion)
}

fn completion_invocation_id(completion: &RunnerCompletion) -> Option<String> {
    completion
        .task_leases
        .first()
        .map(|lease| lease.lease_id.clone())
}

fn supervise_running_invocations(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_tasks: &mut BTreeMap<String, RunningTask>,
    draining_invocations: &mut BTreeMap<String, String>,
) {
    cancel_expired_tick_deadlines(core, pending_cancels, running_tasks);
    let now = Instant::now();
    let expired: Vec<_> = running_tasks
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
            running_tasks,
            draining_invocations,
        );
    }
}

fn cancel_expired_tick_deadlines(
    core: &mut CoreRuntime,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_tasks: &mut BTreeMap<String, RunningTask>,
) {
    let current_step = core.current_step();
    let expired: Vec<_> = running_tasks
        .iter()
        .filter_map(|(task_id, task)| {
            task.deadline_tick
                .is_some_and(|deadline_tick| current_step >= deadline_tick)
                .then(|| task_id.clone())
        })
        .collect();
    for task_id in expired {
        let Some(task) = running_tasks.remove(&task_id) else {
            continue;
        };
        if core.task_status(&task_id) == Some(TaskStatus::Running) {
            let _ = core.cancel_task(&task_id);
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
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_tasks: &mut BTreeMap<String, RunningTask>,
    draining_invocations: &mut BTreeMap<String, String>,
) {
    if draining_invocations.contains_key(invocation_id) {
        return;
    }
    let task_ids: Vec<_> = running_tasks
        .iter()
        .filter_map(|(task_id, task)| {
            (task.invocation_id == invocation_id).then(|| task_id.clone())
        })
        .collect();
    let Some(first_task) = task_ids
        .first()
        .and_then(|task_id| running_tasks.get(task_id))
        .cloned()
    else {
        return;
    };
    for task_id in &task_ids {
        if core.task_status(task_id) == Some(TaskStatus::Running) {
            let _ = core.cancel_task(task_id);
        }
        running_tasks.remove(task_id);
    }
    pending_cancels
        .entry(first_task.runner_id.clone())
        .or_default()
        .push(invocation_id.to_string());
    draining_invocations.insert(invocation_id.to_string(), first_task.runner_id.clone());
    if first_task.worker_id.is_some() {
        isolate_worker(pools, &first_task.execution_class);
    }
}

fn isolate_worker(
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    execution_class: &ExecutionClass,
) {
    if let Some(pool) = pools.get_mut(execution_class) {
        let _ = pool.replace_isolated_worker();
    }
}

fn mark_worker_started(started: WorkerStarted, running_tasks: &mut BTreeMap<String, RunningTask>) {
    let now = Instant::now();
    for task_id in started.task_ids {
        if let Some(task) = running_tasks.get_mut(&task_id) {
            if task.invocation_id == started.invocation_id
                && task.runner_id == started.runner_id
                && task.execution_class == started.execution_class
            {
                task.worker_id = Some(started.worker_id.clone());
                task.worker_started_at = Some(now);
            }
        }
    }
}

fn mark_cancel_requested(invocation_id: &str, running_tasks: &mut BTreeMap<String, RunningTask>) {
    let now = Instant::now();
    for task in running_tasks.values_mut() {
        if task.invocation_id == invocation_id {
            task.cancel_requested_at = Some(now);
        }
    }
}
fn remove_running_tasks(
    completion: &RunnerCompletion,
    running_tasks: &mut BTreeMap<String, RunningTask>,
) {
    for lease in &completion.task_leases {
        running_tasks.remove(&lease.task_id);
    }
}

fn schedule_ready(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    running_tasks: &mut BTreeMap<String, RunningTask>,
) -> RuntimeResult<RunnerLoopReport> {
    let (report, dispatches) = core.claim_ready_dispatches(
        |descriptor, load, current_step, registry_generation| {
            let limits = config
                .runner_limits
                .get(&descriptor.runner_id)
                .unwrap_or(&config.default_runner_limits);
            let pool_slots = pools
                .get(&descriptor.execution_class)
                .map(WorkerPool::available_slots)
                .unwrap_or(0);
            decide_schedule(
                descriptor,
                load,
                current_step,
                registry_generation,
                limits,
                pool_slots,
                config.scheduler_policy.as_ref(),
            )
        },
        None,
    )?;
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
        let deadline_tick = dispatch.ctx.deadline_tick;
        let wall_clock_deadline_at = limits
            .wall_clock_deadline
            .map(|deadline| Instant::now() + deadline);
        let task_ids: Vec<_> = dispatch
            .tasks
            .iter()
            .map(|task| task.task_id.clone())
            .collect();
        let Some(pool) = pools.get(&execution_class) else {
            return Err(host_failure(
                "host.worker.pool_missing",
                format!("execution_class.{execution_class:?}"),
            ));
        };
        pool.send(dispatch)?;
        for task_id in task_ids {
            running_tasks.insert(
                task_id,
                RunningTask {
                    runner_id: runner_id.clone(),
                    invocation_id: invocation_id.clone(),
                    execution_class: execution_class.clone(),
                    deadline_tick,
                    wall_clock_deadline_at,
                    cancel_requested_at: None,
                    worker_id: None,
                    worker_started_at: None,
                },
            );
        }
    }
    Ok(RunnerLoopReport {
        claimed_tasks: report.claimed_tasks,
        completed_tasks: report.completed_tasks,
    })
}
