#![allow(clippy::field_reassign_with_default)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{
    AsyncBatchHandler, AsyncCompletionFuture, Runner, RunnerContext, RuntimeFailure, RuntimeResult,
    RuntimeStopState, ScheduleDecision, TaskHistoryRetention,
};
use mutsuki_runtime_sdk::HostRuntime as SdkHostRuntime;
use serde_json::json;

use crate::{
    HostRuntime, HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, NativeRunner,
    ProcessRunnerSpec, RunnerLimits, RuntimeBootstrapper, ScheduleInput, SchedulerPolicy,
    SpawnedBinaryRunner, TokioAsyncExecutor, runner_manifest,
};

use super::helpers::{descriptor, descriptor_with_class, runtime_profile, test_resource_ref};

struct BlockingObservedRunner {
    descriptor: RunnerDescriptor,
    started_tx: mpsc::Sender<()>,
    release_rx: mpsc::Receiver<()>,
    cancelled: Arc<Mutex<Vec<String>>>,
    disposed: Arc<Mutex<bool>>,
}

struct PendingDrop(Arc<AtomicBool>);

impl Drop for PendingDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct PendingAsyncHandler {
    descriptor: RunnerDescriptor,
    started: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

impl AsyncBatchHandler for PendingAsyncHandler {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, _ctx: RunnerContext, _batch: WorkBatch) -> AsyncCompletionFuture {
        let started = self.started.clone();
        let dropped = self.dropped.clone();
        Box::pin(async move {
            started.store(true, Ordering::SeqCst);
            let _drop = PendingDrop(dropped);
            std::future::pending::<RuntimeResult<CompletionBatch>>().await
        })
    }
}

#[derive(Debug)]
struct NamedScheduler {
    policy_id: &'static str,
}

impl SchedulerPolicy for NamedScheduler {
    fn policy_id(&self) -> &str {
        self.policy_id
    }

    fn decide(&self, input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision> {
        Ok(ScheduleDecision::new(
            self.policy_id,
            input.hard_capacity,
            "test.named",
        ))
    }
}

impl Runner for BlockingObservedRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> mutsuki_runtime_core::RuntimeResult<CompletionBatch> {
        self.started_tx.send(()).unwrap();
        let _ = self.release_rx.recv();
        scalar_completion_batch(&batch, |task| {
            Ok(RunnerResult::completed(task.task_id.clone()))
        })
    }

    fn cancel(&mut self, invocation_id: &str) -> mutsuki_runtime_core::RuntimeResult<()> {
        self.cancelled
            .lock()
            .expect("cancelled mutex poisoned")
            .push(invocation_id.to_string());
        Ok(())
    }

    fn dispose(&mut self) -> mutsuki_runtime_core::RuntimeResult<()> {
        *self.disposed.lock().expect("disposed mutex poisoned") = true;
        Ok(())
    }
}

fn scalar_completion_batch(
    batch: &WorkBatch,
    mut result: impl FnMut(&Task) -> RuntimeResult<RunnerResult>,
) -> RuntimeResult<CompletionBatch> {
    let tasks = match batch.row_payload_tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Ok(CompletionBatch::from_error(batch, error)),
    };
    let mut results = Vec::with_capacity(batch.entries.len());
    for entry in &batch.entries {
        let Some(task) = tasks.iter().find(|task| task.task_id == entry.task_id) else {
            results.push(EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: None,
                error: Some(RuntimeError::new(
                    ERR_TASK_CLAIM_CONFLICT,
                    "host.test.runner",
                    format!("batch.entry.{}", entry.entry_id),
                )),
            });
            continue;
        };
        match result(task) {
            Ok(result) => results.push(EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: Some(result),
                error: None,
            }),
            Err(failure) => results.push(EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: None,
                error: Some(failure.error().clone()),
            }),
        }
    }
    Ok(CompletionBatch::from_results(batch, results))
}

fn wait_for_task_status(runtime: &HostRuntime, task_id: &str, expected: TaskStatus) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1) {
        if runtime.task_status(task_id) == Some(expected.clone()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "task {task_id} did not reach {expected:?}; current status: {:?}",
        runtime.task_status(task_id)
    );
}

fn event_driven_test_deadline() -> Duration {
    if cfg!(windows) {
        Duration::from_secs(1)
    } else {
        Duration::from_millis(150)
    }
}

#[test]
fn host_start_rejects_invalid_worker_pool_capacity_before_returning() {
    let mut invalid_configs = Vec::new();
    let mutations: [fn(&mut HostRuntimeConfig); 7] = [
        |config: &mut HostRuntimeConfig| config.worker_threads = 0,
        |config: &mut HostRuntimeConfig| config.blocking_threads = 0,
        |config: &mut HostRuntimeConfig| config.pool_queue_limit = 0,
        |config: &mut HostRuntimeConfig| config.pool_max_inflight_bytes = 0,
        |config: &mut HostRuntimeConfig| config.max_isolated_workers = 0,
        |config: &mut HostRuntimeConfig| config.management_threads = 0,
        |config: &mut HostRuntimeConfig| config.management_queue_limit = 0,
    ];
    for mutate in mutations {
        let mut config = HostRuntimeConfig::default();
        mutate(&mut config);
        invalid_configs.push(config);
    }

    for config in invalid_configs {
        let mut profile = runtime_profile();
        profile.enabled_plugins.clear();
        let failure =
            match RuntimeBootstrapper::new().into_host_runtime_with_config(profile, config) {
                Ok(_) => panic!("invalid worker pool capacity unexpectedly started the host"),
                Err(failure) => failure,
            };
        assert_eq!(failure.error().code, ERR_RUNTIME_HOST_FAILED);
        assert_eq!(failure.error().route, "host.worker.config");
    }
}

#[test]
fn event_driven_host_sleeps_without_tasks_or_deadlines() {
    let runner_descriptor = descriptor("idle.runner", "idle.work");
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, task| Ok(RunnerResult::completed(task.task_id)),
    )));
    let runtime = host
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                tick_interval: Duration::from_millis(2),
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();

    let before = runtime.drive_state().unwrap();
    std::thread::sleep(Duration::from_millis(40));
    let after = runtime.drive_state().unwrap();

    assert_eq!(before.current_step, 0);
    assert_eq!(after.current_step, before.current_step);
    assert_eq!(after.timed_wakeups, before.timed_wakeups);
    assert_eq!(after.next_required_tick, None);
    assert_eq!(after.next_wake_deadline, None);
}

#[test]
fn event_driven_host_dispatches_submit_and_worker_backlog_without_polling() {
    let runner_descriptor = descriptor("event.runner", "event.work");
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, task| Ok(RunnerResult::completed(task.task_id)),
    )));
    let runtime = host
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitBatch(Box::new(TaskBatch {
            batch_id: "event-batch".into(),
            tick_id: None,
            tasks: vec![
                Task::new("event-1", "event.work", json!({})),
                Task::new("event-2", "event.work", json!({})),
            ],
            resource_plan: None,
        })))
        .unwrap();

    wait_for_task_status(&runtime, "event-1", TaskStatus::Completed);
    wait_for_task_status(&runtime, "event-2", TaskStatus::Completed);
    assert_eq!(runtime.drive_state().unwrap().timed_wakeups, 0);
}

#[test]
fn cancelling_waiting_parent_interrupts_cascaded_running_child_invocation() {
    let parent_descriptor = descriptor("parent.runner", "parent.work");
    let mut child_descriptor = descriptor("child.runner", "child.work");
    child_descriptor.invocation_mode = InvocationMode::AsyncExclusive;
    let child_started = Arc::new(AtomicBool::new(false));
    let child_dropped = Arc::new(AtomicBool::new(false));
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![parent_descriptor.clone(), child_descriptor.clone()],
    ));
    host.register_runner(Box::new(NativeRunner::new(
        parent_descriptor,
        |_ctx, parent| {
            let mut child = Task::new("cascade-child", "child.work", json!({}));
            child.trace_id = parent.trace_id.clone();
            child.correlation_id = parent.correlation_id.clone();
            let child_handle = TaskHandle {
                task_id: child.task_id.clone(),
                protocol_id: child.protocol_id.clone(),
                target_binding_id: None,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: child.trace_id.clone(),
                correlation_id: child.correlation_id.clone(),
            };
            let mut result = RunnerResult::completed(parent.task_id.clone());
            result.status = RunnerStatus::Waiting;
            result.tasks.push(child);
            result.task_await = Some(TaskAwait {
                parent_task_id: parent.task_id.clone(),
                child: child_handle,
                continuation: TaskStepContinuation {
                    continuation: test_resource_ref(
                        "continuation:cascade-parent",
                        "continuation",
                        ResourceSemantic::FrozenValue,
                    ),
                    wake: None,
                    reason: Some("test.await".into()),
                },
                cancel_policy: CancelPolicy::Cascade,
            });
            Ok(result)
        },
    )));
    host.register_async_handler(Arc::new(PendingAsyncHandler {
        descriptor: child_descriptor,
        started: child_started.clone(),
        dropped: child_dropped.clone(),
    }));
    let config = HostRuntimeConfig {
        event_driven: true,
        ..HostRuntimeConfig::default()
    }
    .with_async_executor(Arc::new(
        TokioAsyncExecutor::new(1, 4, 4, 1024 * 1024).unwrap(),
    ));
    let runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    let handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "cascade-parent",
            "parent.work",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        other => panic!("unexpected submit reply: {other:?}"),
    };
    wait_for_task_status(&runtime, "cascade-parent", TaskStatus::Waiting);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1) && !child_started.load(Ordering::SeqCst) {
        let _ = runtime.drive_state();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(child_started.load(Ordering::SeqCst));
    runtime
        .dispatch(HostRuntimeCommand::CancelTask(handle))
        .unwrap();
    wait_for_task_status(&runtime, "cascade-child", TaskStatus::Cancelled);

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1) {
        if child_dropped.load(Ordering::SeqCst) {
            return;
        }
        let _ = runtime.drive_state();
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("cascaded child runner invocation was not cancelled");
}

#[test]
fn completion_subscription_wakes_and_batch_state_returns_terminal_outcome() {
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();
    let completions = runtime.subscribe_task_completions();
    let changes = runtime.subscribe_task_changes();
    let observed_revision = completions.revision();
    let observed_change_revision = changes.revision();
    let handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "completion-notice",
            "raw.input",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("unexpected submit reply: {reply:?}"),
    };

    let revision = completions
        .wait_after(observed_revision)
        .expect("terminal task wakes completion subscription");
    assert!(revision > observed_revision);
    assert!(
        changes
            .wait_after_timeout(observed_change_revision, Duration::from_secs(1))
            .is_some()
    );
    let states = runtime.task_states(vec![handle.clone()]).unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].handle, handle);
    assert_eq!(states[0].status, Some(TaskStatus::Completed));
    assert!(matches!(
        states[0].outcome,
        Some(TaskOutcome::Completed { ref task_id, .. }) if task_id == "completion-notice"
    ));

    let metrics = runtime.metrics();
    assert_eq!(metrics.task_status_queries, 0);
    assert_eq!(metrics.task_state_batch_queries, 1);
    assert!(metrics.completion_notifications >= 1);
}

#[test]
fn wait_task_states_blocks_until_terminal_without_status_polling() {
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();
    let handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "wait-states",
            "raw.input",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("unexpected submit reply: {reply:?}"),
    };

    let states = runtime
        .wait_task_states(vec![handle.clone()], Duration::from_secs(1))
        .unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].handle, handle);
    assert_eq!(states[0].status, Some(TaskStatus::Completed));

    let metrics = runtime.metrics();
    assert_eq!(metrics.task_status_queries, 0);
    assert!(metrics.task_state_batch_queries >= 1);
    assert!(metrics.completion_notifications >= 1);
}

#[test]
fn wait_task_states_times_out_with_current_state_and_wakes_all_waiters_on_completion() {
    let runner_descriptor = descriptor_with_class(
        "wait.blocking.runner",
        "wait.blocking",
        ExecutionClass::Blocking,
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        move |_ctx, task| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(RunnerResult::completed(task.task_id))
        },
    )));
    let runtime = Arc::new(
        host.into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                blocking_threads: 1,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap(),
    );
    let handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "wait-blocking",
            "wait.blocking",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("unexpected submit reply: {reply:?}"),
    };
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let timeout_runtime = runtime.clone();
    let timeout_handle = handle.clone();
    let timeout_waiter = std::thread::spawn(move || {
        timeout_runtime
            .wait_task_states(vec![timeout_handle], Duration::from_millis(20))
            .unwrap()
    });
    let traffic_deadline = Instant::now() + Duration::from_secs(1);
    while !timeout_waiter.is_finished() && Instant::now() < traffic_deadline {
        assert_eq!(
            runtime.task_status("wait-blocking"),
            Some(TaskStatus::Running)
        );
    }
    let timed_out = timeout_waiter.join().unwrap();
    assert_eq!(timed_out[0].status, Some(TaskStatus::Running));
    assert!(timed_out[0].outcome.is_none());

    let waiters = (0..2)
        .map(|_| {
            let runtime = runtime.clone();
            let handle = handle.clone();
            std::thread::spawn(move || {
                runtime
                    .wait_task_states(vec![handle], Duration::from_secs(1))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    release_tx.send(()).unwrap();
    for waiter in waiters {
        let states = waiter.join().unwrap();
        assert_eq!(states[0].status, Some(TaskStatus::Completed));
        assert!(matches!(
            states[0].outcome,
            Some(TaskOutcome::Completed { .. })
        ));
    }
}

#[test]
fn dropping_runtime_closes_completion_waiters() {
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    let completions = runtime.subscribe_task_completions();
    let revision = completions.revision();

    drop(runtime);

    assert_eq!(completions.wait_after(revision), None);
}

#[test]
fn event_driven_host_arms_one_shot_timer_for_future_ready_step() {
    let runner_descriptor = descriptor("timer.runner", "timer.work");
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, task| Ok(RunnerResult::completed(task.task_id)),
    )));
    let runtime = host
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                tick_interval: Duration::from_millis(5),
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();
    let mut task = Task::new("timer-1", "timer.work", json!({}));
    task.ready_at_step = Some(8);
    std::thread::sleep(Duration::from_millis(50));
    let submitted_at = Instant::now();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))
        .unwrap();
    let armed = runtime.drive_state().unwrap();
    assert_eq!(armed.current_step, 1);
    assert_eq!(armed.next_required_tick, Some(8));
    assert!(armed.next_wake_deadline.is_some());
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(runtime.task_status("timer-1"), Some(TaskStatus::Ready));

    wait_for_task_status(&runtime, "timer-1", TaskStatus::Completed);
    assert!(submitted_at.elapsed() < event_driven_test_deadline());
    let completed = runtime.drive_state().unwrap();
    assert_eq!(completed.timed_wakeups, 1);
    assert_eq!(completed.next_required_tick, None);
}

#[test]
fn event_driven_host_enforces_tick_deadline_without_periodic_polling() {
    let runner_descriptor = descriptor_with_class(
        "timer.deadline.runner",
        "timer.deadline",
        ExecutionClass::Blocking,
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let disposed = Arc::new(Mutex::new(false));
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(BlockingObservedRunner {
        descriptor: runner_descriptor,
        started_tx,
        release_rx,
        cancelled: cancelled.clone(),
        disposed,
    }));
    let runtime = host
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                tick_interval: Duration::from_millis(5),
                default_runner_limits: RunnerLimits {
                    deadline_ticks: Some(2),
                    ..RunnerLimits::default()
                },
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();
    let submitted_at = Instant::now();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "timer-deadline-1",
            "timer.deadline",
            json!({}),
        ))))
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    runtime.drive_state().unwrap();
    assert_eq!(
        runtime.task_status("timer-deadline-1"),
        Some(TaskStatus::Running)
    );
    assert!(submitted_at.elapsed() < event_driven_test_deadline());

    release_tx.send(()).unwrap();
    wait_for_task_status(&runtime, "timer-deadline-1", TaskStatus::Cancelled);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1)
        && cancelled
            .lock()
            .expect("cancelled mutex poisoned")
            .is_empty()
    {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(cancelled.lock().unwrap().len(), 1);
}

#[test]
fn host_actor_accepts_work_while_blocking_runner_is_stuck() {
    let blocking_descriptor =
        descriptor_with_class("blocking.runner", "blocking.work", ExecutionClass::Blocking);
    let echo_descriptor = descriptor("echo.runner", "raw.input");
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![blocking_descriptor.clone(), echo_descriptor.clone()],
    ));
    host.register_runner(Box::new(NativeRunner::new(
        blocking_descriptor,
        move |_ctx, tasks| {
            release_rx.recv().unwrap();
            Ok(RunnerResult::completed(tasks.task_id))
        },
    )));
    host.register_runner(Box::new(NativeRunner::new(
        echo_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let runtime = host.into_host_runtime(runtime_profile()).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "blocking-1",
            "blocking.work",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    assert_eq!(runtime.task_status("blocking-1"), Some(TaskStatus::Running));

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "echo-1",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(runtime.task_status("echo-1"), Some(TaskStatus::Completed));
    release_tx.send(()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(
        runtime.task_status("blocking-1"),
        Some(TaskStatus::Completed)
    );
}

#[test]
fn bounded_worker_queue_saturates_without_losing_ready_tasks() {
    let blocker = descriptor_with_class("a.blocker", "queue.block", ExecutionClass::Blocking);
    let echo_b = descriptor_with_class("b.echo", "queue.echo.b", ExecutionClass::Blocking);
    let echo_c = descriptor_with_class("c.echo", "queue.echo.c", ExecutionClass::Blocking);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![blocker.clone(), echo_b.clone(), echo_c.clone()],
    ));
    host.register_runner(Box::new(NativeRunner::new(blocker, move |_ctx, task| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(RunnerResult::completed(task.task_id))
    })));
    for descriptor in [echo_b, echo_c] {
        host.register_runner(Box::new(NativeRunner::new(descriptor, |_ctx, task| {
            Ok(RunnerResult::completed(task.task_id))
        })));
    }
    let config = HostRuntimeConfig {
        blocking_threads: 1,
        pool_queue_limit: 1,
        ..HostRuntimeConfig::default()
    };
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    for (task_id, protocol) in [
        ("queue-block", "queue.block"),
        ("queue-b", "queue.echo.b"),
        ("queue-c", "queue.echo.c"),
    ] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                task_id,
                protocol,
                json!({}),
            ))))
            .unwrap();
    }
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let blocking = runtime
        .worker_pools()
        .unwrap()
        .into_iter()
        .find(|pool| pool.pool_id == "blocking")
        .unwrap();
    assert!(blocking.queued_batches <= 1);
    assert!(
        runtime.task_status("queue-b") == Some(TaskStatus::Ready)
            || runtime.task_status("queue-c") == Some(TaskStatus::Ready)
    );

    release_tx.send(()).unwrap();
    wait_for_status(&mut runtime, "queue-block", TaskStatus::Completed);
    wait_for_status(&mut runtime, "queue-b", TaskStatus::Completed);
    wait_for_status(&mut runtime, "queue-c", TaskStatus::Completed);
}

#[test]
fn task_snapshots_return_live_task_metadata_in_actor_order() {
    let blocking_descriptor = descriptor_with_class(
        "snapshot.blocking.runner",
        "snapshot.blocking",
        ExecutionClass::Blocking,
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![blocking_descriptor.clone()],
    ));
    host.register_runner(Box::new(NativeRunner::new(
        blocking_descriptor,
        move |_ctx, tasks| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(RunnerResult::completed(tasks.task_id))
        },
    )));
    let runtime = host.into_host_runtime(runtime_profile()).unwrap();

    let mut running_task = Task::new("snapshot-running", "snapshot.blocking", json!({}));
    running_task.priority = 7;
    running_task.trace_id = Some("trace-1".into());
    running_task.input_refs = vec!["input:1".into()];
    running_task.required_surfaces = vec!["surface:snapshot".into()];
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(running_task)))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let mut ready_task = Task::new("snapshot-ready", "snapshot.blocking", json!({}));
    ready_task.correlation_id = Some("correlation-1".into());
    ready_task.runner_hint = Some("snapshot.blocking.runner".into());
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(ready_task)))
        .unwrap();

    let snapshots = SdkHostRuntime::task_snapshots(&runtime).unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].task_id, "snapshot-running");
    assert_eq!(snapshots[1].task_id, "snapshot-ready");
    assert_eq!(snapshots[0].status, TaskStatus::Running);
    assert_eq!(snapshots[1].status, TaskStatus::Ready);
    assert_eq!(snapshots[0].priority, 7);
    assert_eq!(
        snapshots[0].trace_id.as_ref().map(|id| id.as_str()),
        Some("trace-1")
    );
    assert_eq!(
        snapshots[0].input_refs,
        vec![mutsuki_runtime_contracts::RefId::from("input:1")]
    );
    assert_eq!(
        snapshots[0].required_surfaces,
        vec![mutsuki_runtime_contracts::SurfaceId::from(
            "surface:snapshot"
        )]
    );
    assert_eq!(
        snapshots[0].claimed_by.as_ref().map(|id| id.as_str()),
        Some("snapshot.blocking.runner")
    );
    assert_eq!(
        snapshots[0].owner_runner.as_ref().map(|id| id.as_str()),
        Some("snapshot.blocking.runner")
    );
    assert!(snapshots[0].lease_id.is_some());
    assert_eq!(
        snapshots[1].runner_hint.as_deref(),
        Some("snapshot.blocking.runner")
    );
    assert_eq!(
        snapshots[1].correlation_id.as_deref(),
        Some("correlation-1")
    );
    assert!(snapshots[1].lease_id.is_none());
    assert!(snapshots[0].created_sequence < snapshots[1].created_sequence);

    release_tx.send(()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
}

#[test]
fn events_after_returns_incremental_runtime_events() {
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "event-task",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    let events = SdkHostRuntime::events_after(&runtime, 0, 128).unwrap();
    assert!(
        events
            .items
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert!(events.items.iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == "task.enqueue"
            && event.subject_id.as_deref() == Some("event-task")
    }));
    assert!(events.items.iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == "task.completed"
            && event.subject_id.as_deref() == Some("event-task")
    }));

    let last_sequence = events.items.last().expect("runtime events exist").sequence;
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "event-task-next",
            "raw.input",
            json!({}),
        ))))
        .unwrap();

    let later_events = SdkHostRuntime::events_after(&runtime, last_sequence, 128).unwrap();
    assert!(!later_events.items.is_empty());
    assert!(
        later_events
            .items
            .iter()
            .all(|event| event.sequence > last_sequence)
    );
    assert!(later_events.items.iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.subject_id.as_deref() == Some("event-task-next")
    }));
}

#[test]
fn host_runtime_exposes_drain_abort_and_statistics_without_changing_task_api() {
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "drain-accepted",
            "raw.input",
            json!({}),
        ))))
        .unwrap();

    assert_eq!(runtime.begin_drain().unwrap(), RuntimeStopState::Draining);
    let rejected = runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "drain-rejected",
            "raw.input",
            json!({}),
        ))))
        .unwrap_err();
    assert_eq!(rejected.error().code, ERR_RUNTIME_NOT_ACCEPTING);
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(
        runtime.task_status("drain-accepted"),
        Some(TaskStatus::Completed)
    );
    let statistics = runtime.statistics().unwrap();
    assert_eq!(statistics.tasks.completed, 1);
    assert_eq!(statistics.tasks.attempts_started, 1);

    let aborted = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    aborted
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "abort-ready",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    assert_eq!(aborted.abort("test abort").unwrap(), 1);
    assert_eq!(aborted.stop_state().unwrap(), RuntimeStopState::Aborted);
    assert_eq!(
        aborted.task_status("abort-ready"),
        Some(TaskStatus::Cancelled)
    );
    let error = aborted
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "abort-rejected",
            "raw.input",
            json!({}),
        ))))
        .unwrap_err();
    assert_eq!(error.error().code, ERR_RUNTIME_ABORTED);
}

#[test]
fn trace_spans_after_returns_incremental_runtime_trace_spans() {
    let mut profile = runtime_profile();
    profile.observability.dispatch_spans = true;
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(profile)
        .unwrap();

    let mut first_task = Task::new("trace-task", "raw.input", json!({}));
    first_task.trace_id = Some("trace-custom".into());
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(first_task)))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    let spans = SdkHostRuntime::trace_spans_after(&runtime, 0, 128).unwrap();
    assert!(!spans.items.is_empty());
    assert!(
        spans
            .items
            .iter()
            .any(|span| { span.name == "runner.run_batch" && span.trace_id == "trace-custom" })
    );
    let next_sequence = spans.next_sequence;
    let empty = SdkHostRuntime::trace_spans_after(&runtime, next_sequence, 128).unwrap();
    assert!(empty.items.is_empty());

    let mut next_task = Task::new("trace-task-next", "raw.input", json!({}));
    next_task.trace_id = Some("trace-next".into());
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(next_task)))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    let later_spans = SdkHostRuntime::trace_spans_after(&runtime, next_sequence, 128).unwrap();
    assert!(later_spans.next_sequence > next_sequence);
    assert!(
        later_spans
            .items
            .iter()
            .any(|span| { span.name == "runner.run_batch" && span.trace_id == "trace-next" })
    );
}

#[test]
fn host_cursor_pages_report_evicted_trace_and_event_records() {
    let mut profile = runtime_profile();
    profile.observability.events =
        ObservabilityOutletProfile::new(2, ObservabilityOverflowPolicy::DropOldest);
    profile.observability.traces =
        ObservabilityOutletProfile::new(1, ObservabilityOverflowPolicy::DropOldest);
    profile.observability.dispatch_spans = true;
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(profile)
        .unwrap();

    for task_id in ["cursor-one", "cursor-two"] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                task_id,
                "raw.input",
                json!({}),
            ))))
            .unwrap();
        runtime
            .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
            .unwrap();
    }

    let events = SdkHostRuntime::events_after(&runtime, 0, 16).unwrap();
    assert!(events.cursor_lost());
    assert!(events.lost > 0);
    assert_eq!(events.items.len(), 2);
    let traces = SdkHostRuntime::trace_spans_after(&runtime, 0, 16).unwrap();
    assert!(traces.cursor_lost());
    assert!(traces.lost > 0);
    assert_eq!(traces.items.len(), 1);
}

#[test]
fn host_runtime_reload_increments_generation_and_adds_runner_surface() {
    let mut runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    let new_descriptor = descriptor("new.runner", "new.input");
    let echo_descriptor = descriptor("echo.runner", "raw.input");
    let mut reload_host = RuntimeBootstrapper::new();
    reload_host.register_manifest(runner_manifest(
        "plugin-a",
        vec![echo_descriptor.clone(), new_descriptor.clone()],
    ));
    reload_host.register_runner(Box::new(NativeRunner::new(
        echo_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    reload_host.register_runner(Box::new(NativeRunner::new(
        new_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));

    let prepared = reload_host.prepare_reload(runtime_profile(), 2).unwrap();
    let decision = SdkHostRuntime::reload(&mut runtime, prepared, Duration::from_secs(1)).unwrap();

    assert_eq!(runtime.host_context().registry_generation(), 2);
    assert!(decision.changes.iter().any(|change| {
        change.surface_id == "runner:new.runner"
            && change.compatibility == SurfaceCompatibility::Additive
    }));

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "new-task",
            "new.input",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(runtime.task_status("new-task"), Some(TaskStatus::Completed));
}

#[test]
fn host_runtime_reload_applies_replaced_runner_limits() {
    let runner_descriptor = descriptor_with_class(
        "reload.limit.runner",
        "reload.limit",
        ExecutionClass::Blocking,
    );
    let (old_started_tx, _old_started_rx) = mpsc::channel();
    let (_old_release_tx, old_release_rx) = mpsc::channel();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(BlockingObservedRunner {
        descriptor: runner_descriptor.clone(),
        started_tx: old_started_tx,
        release_rx: old_release_rx,
        cancelled: Arc::new(Mutex::new(Vec::new())),
        disposed: Arc::new(Mutex::new(false)),
    }));
    let mut initial_limits = std::collections::BTreeMap::new();
    initial_limits.insert(
        runner_descriptor.runner_id.as_str().into(),
        RunnerLimits {
            wall_clock_deadline: Some(Duration::from_millis(20)),
            ..RunnerLimits::default()
        },
    );
    let mut runtime = host
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                runner_limits: initial_limits,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();

    let (new_started_tx, new_started_rx) = mpsc::channel();
    let (new_release_tx, new_release_rx) = mpsc::channel();
    let mut reload_host = RuntimeBootstrapper::new();
    reload_host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    reload_host.register_runner(Box::new(BlockingObservedRunner {
        descriptor: runner_descriptor.clone(),
        started_tx: new_started_tx,
        release_rx: new_release_rx,
        cancelled: Arc::new(Mutex::new(Vec::new())),
        disposed: Arc::new(Mutex::new(false)),
    }));
    let mut replaced_limits = std::collections::BTreeMap::new();
    replaced_limits.insert(
        runner_descriptor.runner_id.as_str().into(),
        RunnerLimits {
            wall_clock_deadline: Some(Duration::from_millis(500)),
            ..RunnerLimits::default()
        },
    );
    let prepared = reload_host
        .prepare_reload_with_runner_limits(runtime_profile(), 2, replaced_limits)
        .unwrap();
    SdkHostRuntime::reload(&mut runtime, prepared, Duration::from_secs(1)).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "reload-limit-task",
            "reload.limit",
            json!({}),
        ))))
        .unwrap();
    new_started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    new_release_tx.send(()).unwrap();
    wait_for_task_status(&runtime, "reload-limit-task", TaskStatus::Completed);
}

#[test]
fn completion_subscription_survives_reload_and_wakes_for_new_generation() {
    let mut runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();
    let completions = runtime.subscribe_task_completions();
    let observed_revision = completions.revision();
    let echo_descriptor = descriptor("echo.runner", "raw.input");
    let mut reload_host = RuntimeBootstrapper::new();
    reload_host.register_manifest(runner_manifest("plugin-a", vec![echo_descriptor.clone()]));
    reload_host.register_runner(Box::new(NativeRunner::new(
        echo_descriptor,
        |_ctx, task| Ok(RunnerResult::completed(task.task_id)),
    )));

    let prepared = reload_host.prepare_reload(runtime_profile(), 2).unwrap();
    SdkHostRuntime::reload(&mut runtime, prepared, Duration::from_secs(1)).unwrap();
    let handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "post-reload-completion",
            "raw.input",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("unexpected submit reply: {reply:?}"),
    };

    let revision = completions
        .wait_after(observed_revision)
        .expect("pre-reload subscription wakes for post-reload terminal task");
    assert!(revision > observed_revision);
    let states = runtime.task_states(vec![handle]).unwrap();
    assert_eq!(states[0].status, Some(TaskStatus::Completed));
    assert!(matches!(
        states[0].outcome,
        Some(TaskOutcome::Completed { ref task_id, .. })
            if task_id == "post-reload-completion"
    ));
}

#[test]
fn host_runtime_reload_waits_for_in_flight_worker_before_swap() {
    let runner_descriptor =
        descriptor_with_class("reload.runner", "reload.work", ExecutionClass::Blocking);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor.clone(),
        move |_ctx, tasks| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(RunnerResult::completed(tasks.task_id))
        },
    )));
    let mut runtime = host.into_host_runtime(runtime_profile()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "reload-running",
            "reload.work",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let mut reload_host = RuntimeBootstrapper::new();
    reload_host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    reload_host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let prepared = reload_host.prepare_reload(runtime_profile(), 2).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let join = std::thread::spawn(move || {
        let result = runtime.reload(prepared, Duration::from_secs(2));
        done_tx.send(result.is_ok()).unwrap();
        runtime
    });

    assert!(done_rx.recv_timeout(Duration::from_millis(80)).is_err());
    release_tx.send(()).unwrap();
    assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    let runtime = join.join().unwrap();
    assert_eq!(runtime.host_context().registry_generation(), 2);
    assert_eq!(
        runtime.task_status("reload-running"),
        Some(TaskStatus::Completed)
    );

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "reload-after",
            "reload.work",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(
        runtime.task_status("reload-after"),
        Some(TaskStatus::Completed)
    );
}

#[test]
fn host_runtime_reload_timeout_preserves_active_generation() {
    let runner_descriptor = descriptor_with_class(
        "reload.timeout.runner",
        "reload.timeout",
        ExecutionClass::Blocking,
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor.clone(),
        move |_ctx, tasks| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(RunnerResult::completed(tasks.task_id))
        },
    )));
    let mut runtime = host.into_host_runtime(runtime_profile()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "reload-timeout",
            "reload.timeout",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let mut reload_host = RuntimeBootstrapper::new();
    reload_host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    reload_host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let prepared = reload_host.prepare_reload(runtime_profile(), 2).unwrap();

    assert!(runtime.reload(prepared, Duration::from_millis(20)).is_err());
    assert_eq!(runtime.host_context().registry_generation(), 1);

    release_tx.send(()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(
        runtime.task_status("reload-timeout"),
        Some(TaskStatus::Completed)
    );
}

#[test]
fn failed_host_runtime_reload_disposes_prepared_runners() {
    struct ObservedDisposeRunner {
        descriptor: RunnerDescriptor,
        disposed: Arc<Mutex<bool>>,
    }

    impl Runner for ObservedDisposeRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.descriptor
        }

        fn run_batch(
            &mut self,
            _ctx: RunnerContext,
            batch: WorkBatch,
        ) -> mutsuki_runtime_core::RuntimeResult<CompletionBatch> {
            scalar_completion_batch(&batch, |task| {
                Ok(RunnerResult::completed(task.task_id.clone()))
            })
        }

        fn dispose(&mut self) -> mutsuki_runtime_core::RuntimeResult<()> {
            *self.disposed.lock().expect("disposed mutex poisoned") = true;
            Ok(())
        }
    }

    struct ObservedEffect(Arc<AtomicBool>);

    impl mutsuki_runtime_sdk::HostEffect for ObservedEffect {
        fn dispose(&mut self) -> mutsuki_runtime_sdk::HostEffectFuture<'_> {
            let disposed = self.0.clone();
            Box::pin(async move {
                disposed.store(true, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    let mut runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    let mut changed_descriptor = descriptor("echo.runner", "raw.input");
    changed_descriptor.input_schema = json!({"changed": true});
    let disposed = Arc::new(Mutex::new(false));
    let effect_disposed = Arc::new(AtomicBool::new(false));
    let mut reload_host = RuntimeBootstrapper::new();
    let mut plugin = mutsuki_runtime_sdk::PluginBuilder::new("plugin-a")
        .host_effect(
            mutsuki_runtime_sdk::HostEffectKind::HostLocal,
            Box::new(ObservedEffect(effect_disposed.clone())),
        )
        .build();
    plugin.manifest = runner_manifest("plugin-a", vec![changed_descriptor.clone()]);
    plugin.runners = vec![Box::new(ObservedDisposeRunner {
        descriptor: changed_descriptor,
        disposed: disposed.clone(),
    })];
    reload_host.register_loaded_plugin(plugin);
    let mut prepared = reload_host.prepare_reload(runtime_profile(), 2).unwrap();
    prepared.plan.contract_surfaces[0].fingerprint = "sha256:breaking".into();

    assert!(runtime.reload(prepared, Duration::from_secs(1)).is_err());
    assert_eq!(runtime.host_context().registry_generation(), 1);
    assert!(*disposed.lock().expect("disposed mutex poisoned"));
    assert!(effect_disposed.load(Ordering::SeqCst));
}

#[test]
fn host_runtime_routes_execution_classes_to_named_worker_pools() {
    let descriptor = descriptor_with_class("script.runner", "script.work", ExecutionClass::Script);
    let observed_thread = Arc::new(Mutex::new(String::new()));
    let observed = observed_thread.clone();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        descriptor,
        move |_ctx, tasks| {
            *observed.lock().expect("observed thread mutex poisoned") = std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string();
            Ok(RunnerResult::completed(tasks.task_id))
        },
    )));
    let runtime = host.into_host_runtime(runtime_profile()).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "script-1",
            "script.work",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(runtime.task_status("script-1"), Some(TaskStatus::Completed));
    assert!(
        observed_thread
            .lock()
            .expect("observed thread mutex poisoned")
            .contains("blocking-worker")
    );
}

#[test]
fn host_worker_failure_marks_task_failed_and_returns_runner() {
    let runner_descriptor = descriptor("flaky.runner", "raw.input");
    let attempts = Arc::new(Mutex::new(0usize));
    let observed = attempts.clone();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        move |_ctx, tasks| {
            let mut attempts = observed.lock().expect("attempts mutex poisoned");
            *attempts += 1;
            if *attempts == 1 {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    "runner.failed",
                    "test.host",
                    "flaky.first_attempt",
                )));
            }
            Ok(RunnerResult::completed(tasks.task_id))
        },
    )));
    let runtime = host.into_host_runtime(runtime_profile()).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-fails",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(runtime.task_status("task-fails"), Some(TaskStatus::Failed));

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-recovers",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(
        runtime.task_status("task-recovers"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(*attempts.lock().expect("attempts mutex poisoned"), 2);
}

#[test]
fn worker_catches_runner_panic_and_keeps_pool_capacity() {
    let runner_descriptor = descriptor("panic.runner", "panic.work");
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        move |_ctx, task| {
            if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("intentional runner panic");
            }
            Ok(RunnerResult::completed(task.task_id))
        },
    )));
    let runtime = host.into_host_runtime(runtime_profile()).unwrap();

    for task_id in ["panic-first", "panic-second"] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                task_id,
                "panic.work",
                json!({}),
            ))))
            .unwrap();
        runtime
            .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
            .unwrap();
    }
    assert_eq!(runtime.task_status("panic-first"), Some(TaskStatus::Failed));
    assert_eq!(
        runtime.task_status("panic-second"),
        Some(TaskStatus::Completed)
    );
    let compute = runtime
        .worker_pools()
        .unwrap()
        .into_iter()
        .find(|pool| pool.pool_id == "compute")
        .unwrap();
    assert_eq!(compute.active_threads, compute.configured_threads);
    assert_eq!(compute.isolated_threads, 0);
}

#[test]
fn native_probe_prevents_side_effects_and_delays_cancelled_publication() {
    let runner_descriptor =
        descriptor_with_class("cancellable.runner", "slow.work", ExecutionClass::Blocking);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let side_effects = Arc::new(AtomicUsize::new(0));
    let observed_side_effects = side_effects.clone();
    let handler_calls = Arc::new(AtomicUsize::new(0));
    let observed_handler_calls = handler_calls.clone();
    let subsequent_probe_clear = Arc::new(AtomicBool::new(false));
    let observed_subsequent_probe = subsequent_probe_clear.clone();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new_cancellable(
        runner_descriptor,
        move |_ctx, task, probe| {
            if observed_handler_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                if !probe.is_cancelled() {
                    observed_side_effects.fetch_add(1, Ordering::SeqCst);
                }
            } else {
                observed_subsequent_probe.store(!probe.is_cancelled(), Ordering::SeqCst);
            }
            Ok(RunnerResult::completed(task.task_id))
        },
    )));
    let runtime = host.into_host_runtime(runtime_profile()).unwrap();
    let completions = runtime.subscribe_task_completions();

    let slow_handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "slow-1",
            "slow.work",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("expected task submitted, got {reply:?}"),
    };
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Running));
    let completion_revision = completions.revision();

    assert_eq!(
        runtime
            .dispatch(HostRuntimeCommand::CancelTask(slow_handle.clone()))
            .unwrap(),
        HostRuntimeReply::TaskCancelled(slow_handle.clone())
    );
    let pending_state = runtime.task_states(vec![slow_handle.clone()]).unwrap();
    assert_eq!(pending_state[0].status, Some(TaskStatus::Running));
    assert_eq!(pending_state[0].outcome, None);
    assert_eq!(completions.revision(), completion_revision);
    assert_eq!(side_effects.load(Ordering::SeqCst), 0);
    assert!(
        !runtime
            .events_after(0, 128)
            .unwrap()
            .items
            .iter()
            .any(|event| {
                event.name == "task.cancelled" && event.subject_id.as_deref() == Some("slow-1")
            })
    );

    release_tx.send(()).unwrap();
    assert!(
        completions
            .wait_after_timeout(completion_revision, Duration::from_secs(1))
            .is_some()
    );
    let cancelled_state = runtime.task_states(vec![slow_handle]).unwrap();
    assert_eq!(cancelled_state[0].status, Some(TaskStatus::Cancelled));
    assert!(matches!(
        cancelled_state[0].outcome,
        Some(TaskOutcome::Cancelled { ref task_id, .. }) if task_id == "slow-1"
    ));
    assert_eq!(side_effects.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime
            .events_after(0, 128)
            .unwrap()
            .items
            .iter()
            .filter(|event| {
                event.name == "task.cancelled" && event.subject_id.as_deref() == Some("slow-1")
            })
            .count(),
        1
    );

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "slow-2",
            "slow.work",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(runtime.task_status("slow-2"), Some(TaskStatus::Completed));
    assert!(subsequent_probe_clear.load(Ordering::SeqCst));
    assert_eq!(handler_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn abort_before_native_worker_start_skips_domain_handler() {
    let blocker_descriptor =
        descriptor_with_class("a.blocker", "abort.blocker", ExecutionClass::Blocking);
    let target_descriptor =
        descriptor_with_class("b.target", "abort.target", ExecutionClass::Blocking);
    let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
    let (release_blocker_tx, release_blocker_rx) = mpsc::channel();
    let target_handler_calls = Arc::new(AtomicUsize::new(0));
    let observed_target_handler_calls = target_handler_calls.clone();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![blocker_descriptor.clone(), target_descriptor.clone()],
    ));
    host.register_runner(Box::new(NativeRunner::new_cancellable(
        blocker_descriptor,
        move |_ctx, task, _probe| {
            blocker_started_tx.send(()).unwrap();
            release_blocker_rx.recv().unwrap();
            Ok(RunnerResult::completed(task.task_id))
        },
    )));
    host.register_runner(Box::new(NativeRunner::new_cancellable(
        target_descriptor,
        move |_ctx, task, _probe| {
            observed_target_handler_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RunnerResult::completed(task.task_id))
        },
    )));
    let config = HostRuntimeConfig {
        blocking_threads: 1,
        pool_queue_limit: 2,
        ..HostRuntimeConfig::default()
    };
    let runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    let blocker_handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "abort-blocker",
            "abort.blocker",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("expected task submitted, got {reply:?}"),
    };
    let target_handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "abort-target",
            "abort.target",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("expected task submitted, got {reply:?}"),
    };
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    blocker_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        runtime.task_status("abort-blocker"),
        Some(TaskStatus::Running)
    );
    assert_eq!(
        runtime.task_status("abort-target"),
        Some(TaskStatus::Running)
    );

    let completions = runtime.subscribe_task_completions();
    let completion_revision = completions.revision();
    assert_eq!(runtime.abort("test abort-before-start").unwrap(), 2);
    let pending_states = runtime
        .task_states(vec![blocker_handle.clone(), target_handle.clone()])
        .unwrap();
    assert!(
        pending_states
            .iter()
            .all(|state| state.status == Some(TaskStatus::Running) && state.outcome.is_none())
    );
    assert_eq!(completions.revision(), completion_revision);
    assert_eq!(target_handler_calls.load(Ordering::SeqCst), 0);

    release_blocker_tx.send(()).unwrap();
    let terminal_states = runtime
        .wait_task_states(vec![blocker_handle, target_handle], Duration::from_secs(1))
        .unwrap();
    assert!(terminal_states.iter().all(|state| {
        state.status == Some(TaskStatus::Cancelled)
            && matches!(state.outcome, Some(TaskOutcome::Cancelled { .. }))
    }));
    assert_eq!(target_handler_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn host_deadline_cancels_running_invocation_and_propagates_cancel() {
    struct DeadlineRunner {
        descriptor: RunnerDescriptor,
        started_tx: mpsc::Sender<()>,
        release_rx: mpsc::Receiver<()>,
        cancelled: Arc<Mutex<Vec<String>>>,
    }

    impl Runner for DeadlineRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.descriptor
        }

        fn run_batch(
            &mut self,
            ctx: RunnerContext,
            batch: WorkBatch,
        ) -> mutsuki_runtime_core::RuntimeResult<CompletionBatch> {
            assert_eq!(ctx.deadline_tick, Some(2));
            assert_eq!(ctx.invocation_id, "batch-1-deadline.runner-1");
            self.started_tx.send(()).unwrap();
            self.release_rx.recv().unwrap();
            scalar_completion_batch(&batch, |task| {
                Ok(RunnerResult::completed(task.task_id.clone()))
            })
        }

        fn cancel(&mut self, invocation_id: &str) -> mutsuki_runtime_core::RuntimeResult<()> {
            self.cancelled
                .lock()
                .expect("cancelled mutex poisoned")
                .push(invocation_id.to_string());
            Ok(())
        }
    }

    let runner_descriptor =
        descriptor_with_class("deadline.runner", "deadline.work", ExecutionClass::Blocking);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(DeadlineRunner {
        descriptor: runner_descriptor,
        started_tx,
        release_rx,
        cancelled: cancelled.clone(),
    }));
    let mut config = HostRuntimeConfig::default();
    config.default_runner_limits = RunnerLimits {
        deadline_ticks: Some(1),
        ..RunnerLimits::default()
    };
    let runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "deadline-1",
            "deadline.work",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(runtime.task_status("deadline-1"), Some(TaskStatus::Running));

    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    assert_eq!(runtime.task_status("deadline-1"), Some(TaskStatus::Running));
    assert!(
        cancelled
            .lock()
            .expect("cancelled mutex poisoned")
            .is_empty()
    );

    release_tx.send(()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(
        runtime.task_status("deadline-1"),
        Some(TaskStatus::Cancelled)
    );
    assert_eq!(
        *cancelled.lock().expect("cancelled mutex poisoned"),
        vec!["batch-1-deadline.runner-1".to_string()]
    );
}

#[test]
fn wall_clock_deadline_isolates_stuck_worker_and_drains_late_completion() {
    let stuck_descriptor =
        descriptor_with_class("stuck.wall.runner", "wall.stuck", ExecutionClass::Blocking);
    let echo_descriptor =
        descriptor_with_class("echo.wall.runner", "wall.echo", ExecutionClass::Blocking);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let disposed = Arc::new(Mutex::new(false));
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![stuck_descriptor.clone(), echo_descriptor.clone()],
    ));
    host.register_runner(Box::new(BlockingObservedRunner {
        descriptor: stuck_descriptor,
        started_tx,
        release_rx,
        cancelled: cancelled.clone(),
        disposed: disposed.clone(),
    }));
    host.register_runner(Box::new(NativeRunner::new(
        echo_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let mut config = HostRuntimeConfig {
        blocking_threads: 1,
        default_runner_limits: RunnerLimits {
            wall_clock_deadline: Some(Duration::from_millis(150)),
            ..RunnerLimits::default()
        },
        ..HostRuntimeConfig::default()
    };
    config.cancel_grace_period = Some(Duration::from_secs(30));
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "wall-stuck-1",
            "wall.stuck",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        runtime.task_status("wall-stuck-1"),
        Some(TaskStatus::Running | TaskStatus::Cancelled)
    ));

    std::thread::sleep(Duration::from_millis(200));
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    assert_eq!(
        runtime.task_status("wall-stuck-1"),
        Some(TaskStatus::Running)
    );

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "wall-echo-1",
            "wall.echo",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(runtime.task_status("wall-echo-1"), Some(TaskStatus::Ready));
    let pools = runtime.worker_pools().unwrap();
    let blocking = pools
        .iter()
        .find(|pool| pool.pool_id == "blocking")
        .unwrap();
    assert_eq!(blocking.configured_threads, 1);
    assert_eq!(blocking.isolated_threads, 1);
    assert!(blocking.degraded);

    release_tx.send(()).unwrap();
    wait_for_dispose(&mut runtime, &disposed);
    wait_for_status(&mut runtime, "wall-echo-1", TaskStatus::Completed);
    assert_eq!(
        runtime.task_status("wall-stuck-1"),
        Some(TaskStatus::Cancelled)
    );
    assert_eq!(
        *cancelled.lock().expect("cancelled mutex poisoned"),
        vec!["batch-1-stuck.wall.runner-1".to_string()]
    );
    assert!(*disposed.lock().expect("disposed mutex poisoned"));
}

#[test]
fn cancel_grace_isolates_stuck_worker_and_recovers_pool_capacity() {
    let stuck_descriptor = descriptor_with_class(
        "stuck.cancel.runner",
        "cancel.stuck",
        ExecutionClass::Blocking,
    );
    let echo_descriptor = descriptor_with_class(
        "echo.cancel.runner",
        "cancel.echo",
        ExecutionClass::Blocking,
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let disposed = Arc::new(Mutex::new(false));
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![stuck_descriptor.clone(), echo_descriptor.clone()],
    ));
    host.register_runner(Box::new(BlockingObservedRunner {
        descriptor: stuck_descriptor,
        started_tx,
        release_rx,
        cancelled: cancelled.clone(),
        disposed: disposed.clone(),
    }));
    host.register_runner(Box::new(NativeRunner::new(
        echo_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let config = HostRuntimeConfig {
        blocking_threads: 1,
        cancel_grace_period: Some(Duration::from_millis(30)),
        ..HostRuntimeConfig::default()
    };
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    let cancel_handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "cancel-stuck-1",
            "cancel.stuck",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("expected task submitted, got {reply:?}"),
    };
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::CancelTask(cancel_handle))
        .unwrap();
    assert_eq!(
        runtime.task_status("cancel-stuck-1"),
        Some(TaskStatus::Running)
    );

    std::thread::sleep(Duration::from_millis(60));
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "cancel-echo-1",
            "cancel.echo",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(
        runtime.task_status("cancel-echo-1"),
        Some(TaskStatus::Ready)
    );

    release_tx.send(()).unwrap();
    wait_for_dispose(&mut runtime, &disposed);
    wait_for_status(&mut runtime, "cancel-echo-1", TaskStatus::Completed);
    assert_eq!(
        runtime.task_status("cancel-stuck-1"),
        Some(TaskStatus::Cancelled)
    );
    assert_eq!(
        *cancelled.lock().expect("cancelled mutex poisoned"),
        vec!["batch-1-stuck.cancel.runner-1".to_string()]
    );
    assert!(*disposed.lock().expect("disposed mutex poisoned"));
}

#[test]
fn worker_health_timeout_cancels_stalled_invocation() {
    let stuck_descriptor = descriptor_with_class(
        "stuck.health.runner",
        "health.stuck",
        ExecutionClass::Blocking,
    );
    let echo_descriptor = descriptor_with_class(
        "echo.health.runner",
        "health.echo",
        ExecutionClass::Blocking,
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let disposed = Arc::new(Mutex::new(false));
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![stuck_descriptor.clone(), echo_descriptor.clone()],
    ));
    host.register_runner(Box::new(BlockingObservedRunner {
        descriptor: stuck_descriptor,
        started_tx,
        release_rx,
        cancelled: cancelled.clone(),
        disposed: disposed.clone(),
    }));
    host.register_runner(Box::new(NativeRunner::new(
        echo_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let config = HostRuntimeConfig {
        blocking_threads: 1,
        worker_health_timeout: Some(Duration::from_millis(30)),
        ..HostRuntimeConfig::default()
    };
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "health-stuck-1",
            "health.stuck",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    std::thread::sleep(Duration::from_millis(60));
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    assert_eq!(
        runtime.task_status("health-stuck-1"),
        Some(TaskStatus::Running)
    );

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "health-echo-1",
            "health.echo",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(
        runtime.task_status("health-echo-1"),
        Some(TaskStatus::Ready)
    );

    release_tx.send(()).unwrap();
    wait_for_dispose(&mut runtime, &disposed);
    wait_for_status(&mut runtime, "health-echo-1", TaskStatus::Completed);
    assert_eq!(
        runtime.task_status("health-stuck-1"),
        Some(TaskStatus::Cancelled)
    );
    assert_eq!(
        *cancelled.lock().expect("cancelled mutex poisoned"),
        vec!["batch-1-stuck.health.runner-1".to_string()]
    );
    assert!(*disposed.lock().expect("disposed mutex poisoned"));
}

#[test]
fn one_hundred_native_hangs_do_not_create_unbounded_replacement_threads() {
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let descriptors = (0..100)
        .map(|index| {
            descriptor_with_class(
                &format!("native.hang.runner.{index}"),
                &format!("native.hang.{index}"),
                ExecutionClass::Blocking,
            )
        })
        .collect::<Vec<_>>();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", descriptors.clone()));
    for descriptor in descriptors {
        let gate = gate.clone();
        host.register_runner(Box::new(NativeRunner::new(
            descriptor,
            move |_ctx, task| {
                let (released, wake) = &*gate;
                let mut released = released.lock().expect("hang gate poisoned");
                while !*released {
                    released = wake.wait(released).expect("hang gate poisoned");
                }
                Ok(RunnerResult::completed(task.task_id))
            },
        )));
    }
    let config = HostRuntimeConfig {
        blocking_threads: 2,
        max_isolated_workers: 2,
        pool_queue_limit: 128,
        default_runner_limits: RunnerLimits {
            wall_clock_deadline: Some(Duration::from_millis(30)),
            ..RunnerLimits::default()
        },
        ..HostRuntimeConfig::default()
    };
    let runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    for index in 0..100 {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                format!("native-hang-task-{index}"),
                format!("native.hang.{index}"),
                json!({}),
            ))))
            .unwrap();
    }
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();

    let blocking = runtime
        .worker_pools()
        .unwrap()
        .into_iter()
        .find(|pool| pool.pool_id == "blocking")
        .unwrap();
    assert_eq!(blocking.configured_threads, 2);
    assert_eq!(blocking.active_threads, 2);
    assert_eq!(blocking.isolated_threads, 2);
    assert!(blocking.degraded);
    assert!(blocking.queued_batches <= 98);

    let (released, wake) = &*gate;
    *released.lock().expect("hang gate poisoned") = true;
    wake.notify_all();
}

#[test]
fn hard_timeout_terminates_process_runner_and_recovers_capacity() {
    let runner_descriptor =
        descriptor_with_class("process.runner", "process.work", ExecutionClass::Script);
    let marker = std::env::temp_dir().join(format!(
        "mutsuki-process-runner-recovery-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let spec = ProcessRunnerSpec {
        command: std::env::current_exe().unwrap(),
        args: vec![
            "--exact".into(),
            "tests::host_runtime::process_runner_helper".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        cwd: None,
        env: std::collections::BTreeMap::from([(
            "MUTSUKI_PROCESS_HELPER_MARKER".into(),
            marker.to_string_lossy().into_owned(),
        )]),
    };
    let runner = SpawnedBinaryRunner::spawn(runner_descriptor.clone(), &spec).unwrap();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor]));
    host.register_runner(Box::new(runner));
    let config = HostRuntimeConfig {
        blocking_threads: 1,
        max_isolated_workers: 1,
        default_runner_limits: RunnerLimits {
            wall_clock_deadline: Some(Duration::from_millis(50)),
            ..RunnerLimits::default()
        },
        ..HostRuntimeConfig::default()
    };
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "process-hang-1",
            "process.work",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    std::thread::sleep(Duration::from_millis(80));
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    wait_for_status(&mut runtime, "process-hang-1", TaskStatus::Cancelled);

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "process-recovers-1",
            "process.work",
            json!({}),
        ))))
        .unwrap();
    wait_for_status(&mut runtime, "process-recovers-1", TaskStatus::Completed);

    let blocking = runtime
        .worker_pools()
        .unwrap()
        .into_iter()
        .find(|pool| pool.pool_id == "blocking")
        .unwrap();
    assert_eq!(blocking.configured_threads, 1);
    assert_eq!(blocking.isolated_threads, 0);
    assert!(!blocking.degraded);
    let _ = std::fs::remove_file(marker);
}

#[test]
#[ignore]
fn process_runner_helper() {
    use std::io::Write;

    use mutsuki_runtime_wire::{
        AnyWireRequest, DEFAULT_WIRE_LIMITS, ProtocolHelloAck, decode_binary_any_request,
        encode_binary_response, read_binary_frame_bytes,
    };

    let Ok(marker) = std::env::var("MUTSUKI_PROCESS_HELPER_MARKER") else {
        return;
    };
    let first_process = !std::path::Path::new(&marker).exists();
    if first_process {
        std::fs::write(&marker, b"started").unwrap();
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    while let Some(frame) = read_binary_frame_bytes(&mut input, DEFAULT_WIRE_LIMITS).unwrap() {
        if first_process {
            loop {
                std::thread::sleep(Duration::from_mins(1));
            }
        }
        let request = decode_binary_any_request(&frame, DEFAULT_WIRE_LIMITS).unwrap();
        let opcode = request.request.opcode();
        let response = match request.request {
            AnyWireRequest::Initialize(initialize) => {
                let ack = ProtocolHelloAck::accept(&initialize.hello, None);
                encode_binary_response(request.request_id, opcode, Ok(&ack), DEFAULT_WIRE_LIMITS)
            }
            AnyWireRequest::RunBatch(run) => {
                let results = run
                    .batch
                    .entries
                    .iter()
                    .map(|entry| EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: Some(RunnerResult::completed(entry.task_id.clone())),
                        error: None,
                    })
                    .collect();
                let completion = CompletionBatch {
                    batch_id: run.batch.batch_id,
                    tick_id: run.batch.tick_id,
                    results,
                    metadata: Vec::new(),
                };
                encode_binary_response(
                    request.request_id,
                    opcode,
                    Ok(&completion),
                    DEFAULT_WIRE_LIMITS,
                )
            }
            AnyWireRequest::CancelRunner(_) | AnyWireRequest::DisposeRunner(_) => {
                encode_binary_response(request.request_id, opcode, Ok(&()), DEFAULT_WIRE_LIMITS)
            }
            other => {
                let error = RuntimeError::new(
                    ERR_RUNTIME_HOST_FAILED,
                    "process_runner_helper",
                    format!("unsupported opcode {:#06x}", other.opcode() as u16),
                );
                encode_binary_response::<()>(
                    request.request_id,
                    opcode,
                    Err(&error),
                    DEFAULT_WIRE_LIMITS,
                )
            }
        }
        .unwrap();
        output.write_all(&response).unwrap();
        output.flush().unwrap();
    }
}

#[test]
fn host_runtime_requires_active_scheduler_policy_instance() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor.clone()]);
    manifest.requires = vec![SurfaceRequirement::new(
        ContractSurfaceKind::SchedulerPolicy,
        "scheduler.fair",
    )];
    manifest.provides.scheduler_policies = vec![SchedulerPolicyDescriptor {
        policy_id: "scheduler.fair".into(),
        version: "1.0.0".into(),
        decision_scope: "dispatch_budget".into(),
    }];
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(manifest);
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::LockedBuiltin;

    let error = host
        .into_host_runtime(profile)
        .err()
        .expect("active scheduler policy without configured instance should fail");

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("capability"),
        Some(&ScalarValue::String(
            "scheduler_policy:scheduler.fair".into()
        ))
    );
}

#[test]
fn host_runtime_rejects_pruned_scheduler_policy_instance() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor.clone()]);
    manifest.provides.scheduler_policies = vec![SchedulerPolicyDescriptor {
        policy_id: "scheduler.fair".into(),
        version: "1.0.0".into(),
        decision_scope: "dispatch_budget".into(),
    }];
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(manifest);
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::LockedBuiltin;
    let mut config = HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(NamedScheduler {
        policy_id: "scheduler.fair",
    });

    let error = host
        .into_host_runtime_with_config(profile, config)
        .err()
        .expect("pruned scheduler policy instance should fail");

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("capability"),
        Some(&ScalarValue::String(
            "scheduler_policy:scheduler.fair".into()
        ))
    );
    assert_eq!(
        error.error().evidence.get("detail"),
        Some(&ScalarValue::String("inactive_load_plan".into()))
    );
}

#[test]
fn host_runtime_registers_only_active_capability_graph_extensions() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor.clone()]);
    manifest.requires = vec![
        SurfaceRequirement::new(ContractSurfaceKind::SchedulerPolicy, "scheduler.fair"),
        SurfaceRequirement::new(ContractSurfaceKind::Workflow, "workflow.linear"),
    ];
    manifest.provides.host_extensions = vec![
        HostExtensionDescriptor {
            extension_id: "host.extension.builtin".into(),
            kind: HostExtensionKind::PluginBackend,
            supported_deployments: vec![PluginDeploymentKind::Builtin],
            reload_policy: "static".into(),
            drain_required: false,
        },
        HostExtensionDescriptor {
            extension_id: "host.extension.abi".into(),
            kind: HostExtensionKind::Bridge,
            supported_deployments: vec![PluginDeploymentKind::Abi],
            reload_policy: "drain_and_swap".into(),
            drain_required: true,
        },
    ];
    manifest.provides.plugin_backends = vec![
        PluginBackendDescriptor {
            backend_id: "plugin.backend.builtin".into(),
            deployment_kind: PluginDeploymentKind::Builtin,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: None,
            bridge_id: None,
        },
        PluginBackendDescriptor {
            backend_id: "plugin.backend.abi".into(),
            deployment_kind: PluginDeploymentKind::Abi,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: Some("mutsuki.codec.typed-msgpack.v1".into()),
            bridge_id: Some("bridge.abi.binary".into()),
        },
    ];
    manifest.provides.codecs = vec![CodecDescriptor {
        codec_id: "mutsuki.codec.typed-msgpack.v1".into(),
        media_type: "application/vnd.mutsuki.msgpack".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    }];
    manifest.provides.bridges = vec![BridgeDescriptor {
        bridge_id: "bridge.abi.binary".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        codec_ids: vec!["mutsuki.codec.typed-msgpack.v1".into()],
        drain_policy: "connection_drain".into(),
    }];
    manifest.provides.scheduler_policies = vec![SchedulerPolicyDescriptor {
        policy_id: "scheduler.fair".into(),
        version: "1.0.0".into(),
        decision_scope: "dispatch_budget".into(),
    }];
    manifest.provides.workflows = vec![WorkflowDescriptor {
        workflow_id: "workflow.linear".into(),
        state_resource_kind: "workflow.instance".into(),
        runner_protocol_id: "workflow.linear.run".into(),
        reload_policy: "state_resource_handoff".into(),
    }];
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(manifest);
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::LockedBuiltin;
    profile.allow_hot_reload = false;
    let mut config = HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(NamedScheduler {
        policy_id: "scheduler.fair",
    });
    let runtime = host.into_host_runtime_with_config(profile, config).unwrap();

    assert!(
        runtime
            .capabilities()
            .require_host_extension("host.extension.builtin")
            .is_ok()
    );
    assert!(
        runtime
            .host_context()
            .capability_broker()
            .require_host_extension("host.extension.builtin")
            .is_ok()
    );
    assert!(
        runtime
            .capabilities()
            .require_plugin_backend("plugin.backend.builtin")
            .is_ok()
    );
    assert!(
        runtime
            .capabilities()
            .require_scheduler_policy("scheduler.fair")
            .is_ok()
    );
    assert!(
        runtime
            .capabilities()
            .require_workflow("workflow.linear")
            .is_ok()
    );

    assert_pruned_capability(
        runtime
            .capabilities()
            .require_plugin_backend("plugin.backend.abi"),
        "plugin_backend:plugin.backend.abi",
    );
    assert_pruned_capability(
        runtime.capabilities().require_bridge("bridge.abi.binary"),
        "bridge:bridge.abi.binary",
    );
    assert_pruned_capability(
        runtime
            .capabilities()
            .require_codec("mutsuki.codec.typed-msgpack.v1"),
        "codec:mutsuki.codec.typed-msgpack.v1",
    );
    assert_pruned_capability(
        runtime
            .capabilities()
            .require_host_extension("host.extension.abi"),
        "host_extension:host.extension.abi",
    );
}

#[test]
fn host_runtime_sdk_context_submits_tasks_and_requests_shutdown() {
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    assert_eq!(runtime.host_context().profile_id(), "default");
    assert_eq!(runtime.host_context().registry_generation(), 1);
    assert!(runtime.host_context().services().is_frozen());

    let handle = SdkHostRuntime::submit_task(
        &runtime,
        Task::new("sdk-host-task", "raw.input", json!({"source": "sdk"})),
    )
    .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(handle.task_id, "sdk-host-task");
    assert!(matches!(
        SdkHostRuntime::task_outcome(&runtime, &handle).unwrap(),
        Some(TaskOutcome::Completed { task_id, .. }) if task_id == "sdk-host-task"
    ));
    SdkHostRuntime::request_shutdown(&runtime, "test.shutdown").unwrap();
    assert!(runtime.host_context().shutdown().is_shutdown_requested());
}

#[test]
fn host_context_resource_registry_rejects_unknown_provider() {
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let error = runtime
        .host_context()
        .resource_registry()
        .create_blob_resource("missing.provider", "image.v1", vec![1, 2, 3])
        .unwrap_err();

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("provider_id"),
        Some(&ScalarValue::String("missing.provider".into()))
    );
}

#[test]
fn host_runtime_applies_bounded_terminal_history_policy() {
    let config = HostRuntimeConfig {
        task_history_retention: Some(TaskHistoryRetention::new(2, 4)),
        ..HostRuntimeConfig::default()
    };
    let runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    let tasks = (0..3)
        .map(|index| Task::new(format!("history-{index}"), "raw.input", json!({})))
        .collect();
    runtime
        .dispatch(HostRuntimeCommand::SubmitBatch(Box::new(TaskBatch {
            batch_id: "bounded-history".into(),
            tick_id: None,
            tasks,
            resource_plan: None,
        })))
        .unwrap();
    for _ in 0..10 {
        runtime
            .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
            .unwrap();
        if runtime.task_status("history-2") == Some(TaskStatus::Completed) {
            break;
        }
    }

    let snapshots = runtime.task_snapshots().unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(runtime.task_status("history-0"), None);
    assert_eq!(
        runtime.task_status("history-1"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("history-2"),
        Some(TaskStatus::Completed)
    );
}

fn assert_pruned_capability<T>(result: mutsuki_runtime_core::RuntimeResult<&T>, capability: &str) {
    let error = match result {
        Ok(_) => panic!("pruned capability should be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("capability"),
        Some(&ScalarValue::String(capability.into()))
    );
    assert_eq!(
        error.error().evidence.get("detail"),
        Some(&ScalarValue::String("inactive_load_plan".into()))
    );
}

fn wait_for_dispose(runtime: &mut HostRuntime, disposed: &Arc<Mutex<bool>>) {
    for _ in 0..10 {
        if *disposed.lock().expect("disposed mutex poisoned") {
            return;
        }
        runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_status(runtime: &mut HostRuntime, task_id: &str, expected: TaskStatus) {
    for _ in 0..10 {
        if runtime.task_status(task_id) == Some(expected.clone()) {
            return;
        }
        runtime
            .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
}
