//! Running invocation completion, cancellation deadlines, health supervision, and isolation.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use mutsuki_runtime_contracts::{AsyncInvocationHandle, ExecutionClass, TaskHandle, TaskStatus};
use mutsuki_runtime_core::{
    CoreRuntime, RunnerCompletion, RunnerIsolation, RunnerLoopReport, RunnerManagementHandle,
    RuntimeResult,
};

use crate::async_executor::AsyncExecutorEvent;
use crate::error::host_failure;
use crate::host::HostRuntimeConfig;
use crate::management::ManagementExecutor;
use crate::resource_router;
use crate::worker::{WorkerPools, WorkerStarted};

use super::cancellation::request_running_cancel;
#[derive(Clone)]
/// Actor bookkeeping for one task entry in an active batch. The task and lease
/// authority remains in `CoreRuntime`.
pub(super) struct RunningBatch {
    pub(super) runner_id: String,
    pub(super) invocation_id: String,
    pub(super) batch_id: String,
    pub(super) execution_class: ExecutionClass,
    pub(super) handle: TaskHandle,
    pub(super) deadline_tick: Option<u64>,
    pub(super) wall_clock_deadline_at: Option<Instant>,
    pub(super) cancel_requested_at: Option<Instant>,
    pub(super) worker_id: Option<String>,
    pub(super) worker_started_at: Option<Instant>,
    pub(super) isolation: RunnerIsolation,
    pub(super) management: Option<Arc<dyn RunnerManagementHandle>>,
    pub(super) async_handle: Option<AsyncInvocationHandle>,
}

/// Invocation isolated from new work until its late completion is disposed or
/// its hard-process runner has recovered.
pub(super) struct DrainingInvocation {
    pub(super) runner_id: String,
    pub(super) recover_after_termination: bool,
}

fn apply_pending_cancels(
    completion: &mut RunnerCompletion,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) {
    let Some(runner) = completion.runner.as_mut() else {
        return;
    };
    let runner_id = runner.descriptor().runner_id.clone();
    let Some(invocation_ids) = pending_cancels.remove(&runner_id) else {
        return;
    };
    for invocation_id in invocation_ids {
        let _ = runner.cancel(&invocation_id);
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

/// Applies cancellation/disposal fencing before routing a worker completion
/// back through the authoritative core completion path.
pub(super) fn handle_worker_completion(
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
            let runner = completion.runner.as_mut().ok_or_else(|| {
                host_failure(
                    "host.runner.hard_timeout",
                    "async handler cannot use hard-process recovery",
                )
            })?;
            runner.recover_after_hard_termination()?;
            completion.result = Err(host_failure(
                "host.runner.hard_timeout",
                format!("runner {} was terminated and recovered", draining.runner_id),
            ));
            return core.complete_runner_dispatch(completion);
        }
        if let Some(mut runner) = completion.runner.take() {
            let _ = runner.cancel(&invocation_id);
            if let Some(management) = runner.management_handle() {
                let _ = management.dispose();
            } else {
                let _ = runner.dispose();
            }
        }
        remove_running_batch_entries(&completion, running_batches_by_task);
        return core.complete_runner_dispatch(completion);
    }

    apply_pending_cancels(&mut completion, pending_cancels);
    remove_running_batch_entries(&completion, running_batches_by_task);
    core.complete_runner_dispatch(completion)
}

pub(super) fn handle_async_event(
    event: AsyncExecutorEvent,
    core: &mut CoreRuntime,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
) -> RuntimeResult<RunnerLoopReport> {
    let (invocation, result) = match event {
        AsyncExecutorEvent::ResourceCompleted {
            invocation: _,
            reply,
            result,
        } => {
            let result = match *result {
                Ok(value) => {
                    resource_router::sync_async_resource_reply(core, &value).map(|()| value)
                }
                Err(failure) => Err(failure),
            };
            let _ = reply.send(result);
            return Ok(RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: 0,
            });
        }
        AsyncExecutorEvent::ResourceTimedOut { invocation, reply } => {
            let _ = reply.send(Err(host_failure(
                "host.async_resource.timeout",
                format!(
                    "async resource invocation {} timed out",
                    invocation.invocation_id
                ),
            )));
            return Ok(RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: 0,
            });
        }
        AsyncExecutorEvent::ResourcePanicked { invocation, reply } => {
            let _ = reply.send(Err(host_failure(
                "host.async_resource.panic",
                format!(
                    "async resource invocation {} panicked",
                    invocation.invocation_id
                ),
            )));
            return Ok(RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: 0,
            });
        }
        AsyncExecutorEvent::Started(invocation) => {
            let now = Instant::now();
            for task_id in &invocation.task_ids {
                if let Some(task) = running_batches_by_task.get_mut(task_id)
                    && task.invocation_id == invocation.invocation_id
                {
                    task.worker_started_at = Some(now);
                }
            }
            return Ok(RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: 0,
            });
        }
        AsyncExecutorEvent::Completed { invocation, result } => (invocation, result),
        AsyncExecutorEvent::TimedOut(invocation) => {
            let failure = host_failure(
                "host.async_executor.timeout",
                format!("async invocation {} timed out", invocation.invocation_id),
            );
            (invocation, Err(failure))
        }
        AsyncExecutorEvent::Panicked(invocation) => {
            let failure = host_failure(
                "host.async_executor.panic",
                format!("async invocation {} panicked", invocation.invocation_id),
            );
            (invocation, Err(failure))
        }
        AsyncExecutorEvent::Cancelled(invocation) => {
            let failure = host_failure(
                "host.async_executor.cancelled",
                format!(
                    "async invocation {} was cancelled",
                    invocation.invocation_id
                ),
            );
            (invocation, Err(failure))
        }
    };
    handle_worker_completion(
        RunnerCompletion {
            runner: None,
            task_leases: invocation.task_leases,
            batch_id: invocation.batch_id,
            expected_entries: invocation.expected_entries,
            result,
        },
        core,
        pending_cancels,
        running_batches_by_task,
        draining_invocations,
    )
}

/// Enforces tick, wall-clock, cancellation-grace, and worker-health deadlines
/// without synthesizing replacement completions.
pub(super) fn supervise_running_invocations(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut WorkerPools,
    management: &ManagementExecutor,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
    draining_invocations: &mut BTreeMap<String, DrainingInvocation>,
) {
    cancel_expired_tick_deadlines(
        core,
        config,
        management,
        pending_cancels,
        running_batches_by_task,
    );
    let now = Instant::now();
    let expired: Vec<_> = running_batches_by_task
        .values()
        .filter(|task| {
            (task.async_handle.is_none()
                && task
                    .wall_clock_deadline_at
                    .is_some_and(|deadline| now >= deadline))
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
            config,
            pools,
            pending_cancels,
            running_batches_by_task,
            draining_invocations,
        );
    }
}

fn cancel_expired_tick_deadlines(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    management: &ManagementExecutor,
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
        let Some(task) = running_batches_by_task.get(&task_id).cloned() else {
            continue;
        };
        if task_status(core, &task_id) == Some(TaskStatus::Running) {
            let _ = core.cancel_task_handle(&task.handle);
            if !cancel_async_invocation(&task.invocation_id, config, running_batches_by_task) {
                request_running_cancel(
                    &task.invocation_id,
                    management,
                    running_batches_by_task,
                    pending_cancels,
                );
            }
        }
    }
}

fn isolate_invocation(
    invocation_id: &str,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
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
    if first_task.async_handle.is_some() {
        for task_id in &task_ids {
            if let Some(task) = running_batches_by_task.get(task_id)
                && task_status(core, task_id) == Some(TaskStatus::Running)
            {
                let _ = core.cancel_task_handle(&task.handle);
            }
        }
        let _ = cancel_async_invocation(invocation_id, config, running_batches_by_task);
        return;
    }
    for task_id in &task_ids {
        if let Some(task) = running_batches_by_task.get(task_id)
            && task_status(core, task_id) == Some(TaskStatus::Running)
        {
            let _ = core.cancel_task_handle(&task.handle);
        }
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

pub(super) fn task_status(core: &CoreRuntime, task_id: &str) -> Option<TaskStatus> {
    core.tasks()
        .get(task_id)
        .map(|record| record.status.clone())
}

pub(super) fn mark_worker_started(
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

fn remove_running_batch_entries(
    completion: &RunnerCompletion,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) {
    for lease in &completion.task_leases {
        running_batches_by_task.remove(&lease.task_id);
    }
}

pub(super) fn cancel_async_invocation(
    invocation_id: &str,
    config: &HostRuntimeConfig,
    running_batches_by_task: &mut BTreeMap<String, RunningBatch>,
) -> bool {
    let handle = running_batches_by_task
        .values()
        .find(|task| task.invocation_id == invocation_id)
        .and_then(|task| task.async_handle.clone());
    let Some(handle) = handle else {
        return false;
    };
    config
        .async_executor
        .as_ref()
        .is_some_and(|executor| executor.cancel(&handle).unwrap_or(false))
}
