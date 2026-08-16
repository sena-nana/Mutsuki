//! Completion revision, task waiting, reload drain, and runner disposal contracts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    CompletionBatch, RunnerId, TaskHandle, TaskId, TaskStatus, WorkBatch,
};
use mutsuki_runtime_core::{
    CoreRuntime, ReloadDecision, Runner, RunnerIsolation, RunnerManagementHandle, RuntimeResult,
};

use crate::PreparedRuntimeReload;
use crate::commands::HostTaskState;
use crate::error::host_failure;
use crate::host::{HostRuntimeConfig, TaskCompletionHub};
use crate::management::ManagementExecutor;
use crate::scheduler::validate_runner_limits;
use crate::worker::WorkerPools;

use super::cancellation::queue_management_retry;
use super::mailbox::{ActorReceiver, CoreActorMsg};
use super::supervision::{
    DrainingInvocation, RunningBatch, handle_async_event, handle_worker_completion,
    mark_worker_started, supervise_running_invocations, task_status,
};
/// Actor-owned waiter resolved only from authoritative `CoreRuntime` task state.
pub(super) struct PendingTaskWait {
    pub(super) handles: Vec<TaskHandle>,
    pub(super) deadline: Instant,
    pub(super) reply: mpsc::Sender<RuntimeResult<Vec<HostTaskState>>>,
}

pub(super) fn terminal_revision(core: &CoreRuntime) -> u64 {
    let statistics = core.tasks().statistics();
    (statistics.completed
        + statistics.failed
        + statistics.cancelled
        + statistics.expired
        + statistics.dead_letter) as u64
        + statistics.terminal_records_evicted
}

/// Publishes a monotonic terminal revision and resolves registered waiters after
/// the authoritative task store advances.
pub(super) fn publish_terminal_changes(
    core: &CoreRuntime,
    previous_revision: &mut u64,
    completion_hub: &TaskCompletionHub,
    pending_task_waits: &mut Vec<PendingTaskWait>,
) {
    let revision = terminal_revision(core);
    if revision > *previous_revision {
        *previous_revision = revision;
        completion_hub.publish(revision);
        resolve_pending_task_waits(core, pending_task_waits, false);
    }
}

pub(super) fn register_task_wait(
    core: &CoreRuntime,
    pending_task_waits: &mut Vec<PendingTaskWait>,
    handles: Vec<TaskHandle>,
    deadline: Instant,
    reply: mpsc::Sender<RuntimeResult<Vec<HostTaskState>>>,
) {
    let states = task_states(core, &handles);
    let ready = match states.as_ref() {
        Ok(states) => all_task_states_terminal(states) || Instant::now() >= deadline,
        Err(_) => true,
    };
    if ready {
        let _ = reply.send(states);
    } else {
        pending_task_waits.push(PendingTaskWait {
            handles,
            deadline,
            reply,
        });
    }
}

pub(super) fn resolve_pending_task_waits(
    core: &CoreRuntime,
    pending_task_waits: &mut Vec<PendingTaskWait>,
    force: bool,
) {
    let now = Instant::now();
    let mut index = 0;
    while index < pending_task_waits.len() {
        let states = task_states(core, &pending_task_waits[index].handles);
        let ready = force
            || now >= pending_task_waits[index].deadline
            || match states.as_ref() {
                Ok(states) => all_task_states_terminal(states),
                Err(_) => true,
            };
        if ready {
            let pending = pending_task_waits.swap_remove(index);
            let _ = pending.reply.send(states);
        } else {
            index += 1;
        }
    }
}

pub(super) fn task_states(
    core: &CoreRuntime,
    handles: &[TaskHandle],
) -> RuntimeResult<Vec<HostTaskState>> {
    handles
        .iter()
        .cloned()
        .map(|handle| {
            Ok(HostTaskState {
                status: core.task_handle_status(&handle),
                outcome: core.task_handle_outcome(&handle)?,
                handle,
            })
        })
        .collect()
}

fn all_task_states_terminal(states: &[HostTaskState]) -> bool {
    states.iter().all(|state| {
        matches!(
            state.status,
            Some(
                TaskStatus::Completed
                    | TaskStatus::Failed
                    | TaskStatus::Cancelled
                    | TaskStatus::Expired
                    | TaskStatus::DeadLetter
            )
        )
    })
}

/// Drains affected invocations before atomically applying the prepared reload;
/// runner limits are rolled back when the core reload fails.
pub(super) fn reload_runtime(
    prepared: PreparedRuntimeReload,
    drain_timeout: Duration,
    core: &mut CoreRuntime,
    config: &mut HostRuntimeConfig,
    pools: &mut WorkerPools,
    management: &ManagementExecutor,
    rx: &ActorReceiver,
    pending_cancels: &mut BTreeMap<RunnerId, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<TaskId, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
) -> RuntimeResult<ReloadDecision> {
    let affected_runner_ids = prepared.affected_plugins.as_ref().map(|affected_plugins| {
        core.registry_snapshot()
            .runners
            .into_iter()
            .filter(|runner| affected_plugins.contains(&runner.plugin_id))
            .map(|runner| runner.runner_id)
            .collect::<BTreeSet<_>>()
    });
    drain_for_reload(
        core,
        config,
        pools,
        management,
        rx,
        pending_cancels,
        running_batches_by_task,
        draining_invocations,
        drain_timeout,
        affected_runner_ids.as_ref(),
    )?;
    let PreparedRuntimeReload {
        plan,
        runners,
        async_handlers,
        runner_limits,
        affected_plugins,
        ..
    } = prepared;
    let previous_runner_limits = config.runner_limits.clone();
    if let Some(runner_limits) = runner_limits {
        validate_runner_limits(&config.default_runner_limits, &runner_limits)?;
        config.runner_limits = runner_limits;
    }
    let runners = runners
        .into_iter()
        .map(|runner| Box::new(DisposeOnDropRunner::new(runner)) as Box<dyn Runner>)
        .collect();
    let result = match affected_plugins {
        Some(affected_plugins) => core.reload_targeted_with_async_handlers(
            plan,
            runners,
            async_handlers,
            affected_plugins,
        ),
        None => core.reload_with_async_handlers(plan, runners, async_handlers),
    };
    if result.is_err() {
        config.runner_limits = previous_runner_limits;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn drain_for_reload(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    management: &ManagementExecutor,
    rx: &ActorReceiver,
    pending_cancels: &mut BTreeMap<RunnerId, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<TaskId, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
    drain_timeout: Duration,
    affected_runner_ids: Option<&BTreeSet<RunnerId>>,
) -> RuntimeResult<()> {
    let started_at = Instant::now();
    loop {
        supervise_running_invocations(
            core,
            config,
            pools,
            management,
            pending_cancels,
            running_batches_by_task,
            draining_invocations,
        );
        let running_count = running_batches_by_task
            .values()
            .filter(|batch| {
                affected_runner_ids.is_none_or(|runner_ids| runner_ids.contains(&batch.runner_id))
            })
            .count();
        if running_count == 0 {
            return Ok(());
        }
        let elapsed = started_at.elapsed();
        if elapsed >= drain_timeout {
            return Err(host_failure(
                "host.reload.drain_timeout",
                format!(
                    "timed out waiting for {} running batch entry/entries to drain",
                    running_count
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
            Ok(CoreActorMsg::AsyncEvent(event)) => {
                let _ = handle_async_event(
                    event,
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
            Ok(CoreActorMsg::ManagementFailed {
                runner_id,
                invocation_id,
            }) => queue_management_retry(
                runner_id,
                invocation_id,
                running_batches_by_task,
                pending_cancels,
            ),
            Ok(CoreActorMsg::TaskStatus(task_id, reply_tx)) => {
                let _ = reply_tx.send(task_status(core, &task_id));
            }
            Ok(CoreActorMsg::WaitTaskStates { reply, .. }) => {
                let _ = reply.send(Err(host_failure(
                    "host.actor.mailbox",
                    "task waits must use the control mailbox",
                )));
            }
            Ok(CoreActorMsg::Command(_, reply_tx)) => {
                let _ = reply_tx.send(Err(host_failure(
                    "host.reload.busy",
                    "runtime reload is draining active work",
                )));
            }
            Ok(CoreActorMsg::AsyncResourceCommand(_, reply_tx)) => {
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

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        self.inner.management_handle()
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
