use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{AsyncBatchHandler, AsyncCompletionFuture, RunnerContext};
use serde_json::json;

use crate::{
    HostRuntime, HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, RuntimeBootstrapper,
    TokioAsyncExecutor, runner_manifest,
};

use super::helpers::{descriptor, runtime_profile};

struct DelayedAsyncHandler {
    descriptor: RunnerDescriptor,
    delay: Duration,
    invocations: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

struct GatedAsyncHandler {
    descriptor: RunnerDescriptor,
    started: Arc<AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

impl AsyncBatchHandler for GatedAsyncHandler {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, _ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        let started = self.started.clone();
        let release = self.release.clone();
        Box::pin(async move {
            started.store(1, Ordering::SeqCst);
            release.notified().await;
            Ok(CompletionBatch::from_results(
                &batch,
                batch
                    .entries
                    .iter()
                    .map(|entry| EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: Some(RunnerResult::completed(entry.task_id.clone())),
                        error: None,
                    })
                    .collect(),
            ))
        })
    }
}

impl AsyncBatchHandler for DelayedAsyncHandler {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, _ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let delay = self.delay;
        let active = self.active.clone();
        let max_active = self.max_active.clone();
        Box::pin(async move {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_active.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(CompletionBatch::from_results(
                &batch,
                batch
                    .entries
                    .iter()
                    .map(|entry| EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: Some(RunnerResult::completed(entry.task_id.clone())),
                        error: None,
                    })
                    .collect(),
            ))
        })
    }
}

fn async_descriptor(max_inflight: usize) -> RunnerDescriptor {
    let mut descriptor = descriptor("async.io", "async.io.work");
    descriptor.execution_class = ExecutionClass::Io;
    descriptor.invocation_mode = InvocationMode::AsyncReentrant;
    descriptor.concurrency = RunnerConcurrency::Reentrant {
        max_inflight_batches: max_inflight,
        max_inflight_entries: max_inflight,
    };
    descriptor.batch.preferred_batch_size = 1;
    descriptor.batch.max_batch_entries = 1;
    descriptor.batch.max_entry_concurrency = 1;
    descriptor.batch.max_inflight_batches = max_inflight;
    descriptor
}

fn async_host(
    task_capacity: usize,
    delay: Duration,
    with_executor: bool,
) -> (HostRuntime, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let descriptor = async_descriptor(task_capacity);
    let invocations = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(runner_manifest("plugin-a", vec![descriptor.clone()]));
    bootstrapper.register_async_handler(Arc::new(DelayedAsyncHandler {
        descriptor,
        delay,
        invocations: invocations.clone(),
        active,
        max_active: max_active.clone(),
    }));
    let mut config = HostRuntimeConfig {
        event_driven: true,
        ..HostRuntimeConfig::default()
    };
    config.default_runner_limits.max_running = task_capacity;
    config.default_runner_limits.max_inflight = task_capacity;
    config.default_runner_limits.max_waiting = task_capacity;
    if with_executor {
        config = config.with_async_executor(Arc::new(
            TokioAsyncExecutor::new(2, task_capacity, task_capacity, 64 * 1024 * 1024).unwrap(),
        ));
    }
    (
        bootstrapper
            .into_host_runtime_with_config(runtime_profile(), config)
            .unwrap(),
        invocations,
        max_active,
    )
}

fn submit_tasks(runtime: &HostRuntime, count: usize) -> Vec<TaskHandle> {
    let tasks = (0..count)
        .map(|index| Task::new(format!("async-{index}"), "async.io.work", json!({})))
        .collect();
    match runtime
        .dispatch(HostRuntimeCommand::SubmitBatch(Box::new(TaskBatch {
            batch_id: "async-acceptance".into(),
            tick_id: None,
            tasks,
            resource_plan: None,
        })))
        .unwrap()
    {
        HostRuntimeReply::TaskBatchSubmitted(handles) => handles,
        other => panic!("unexpected submit reply: {other:?}"),
    }
}

fn wait_for_all(runtime: &HostRuntime, handles: &[TaskHandle], status: TaskStatus) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let states = runtime.task_states(handles.to_vec()).unwrap();
        if states
            .iter()
            .all(|state| state.status.as_ref() == Some(&status))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let states = runtime.task_states(handles.to_vec()).unwrap();
    let terminal = states
        .iter()
        .filter(|state| state.status.as_ref() == Some(&status))
        .count();
    panic!("only {terminal}/{} tasks reached {status:?}", handles.len());
}

#[test]
fn one_thousand_delayed_io_tasks_share_two_executor_threads_without_repolling() {
    let (runtime, invocations, max_active) = async_host(1_000, Duration::from_millis(20), true);
    let handles = submit_tasks(&runtime, 1_000);
    wait_for_all(&runtime, &handles, TaskStatus::Completed);

    assert_eq!(invocations.load(Ordering::SeqCst), 1_000);
    assert!(max_active.load(Ordering::SeqCst) > 2);
    match runtime.dispatch(HostRuntimeCommand::AsyncExecutor).unwrap() {
        HostRuntimeReply::AsyncExecutor(Some(snapshot)) => {
            assert_eq!(snapshot.configured_threads, 2);
            assert_eq!(snapshot.running_invocations, 0);
            assert_eq!(snapshot.running_entries, 0);
        }
        other => panic!("unexpected async executor reply: {other:?}"),
    }
}

#[test]
fn async_runner_without_host_executor_fails_structurally() {
    let (runtime, invocations, _) = async_host(1, Duration::ZERO, false);
    let handles = submit_tasks(&runtime, 1);
    wait_for_all(&runtime, &handles, TaskStatus::Failed);
    assert_eq!(invocations.load(Ordering::SeqCst), 0);

    match runtime
        .dispatch(HostRuntimeCommand::TaskOutcome(handles[0].clone()))
        .unwrap()
    {
        HostRuntimeReply::TaskOutcome(Some(TaskOutcome::Failed { error, .. })) => {
            assert_eq!(error.route, "host.async_executor.unavailable")
        }
        other => panic!("unexpected task outcome reply: {other:?}"),
    }
}

#[test]
fn async_runner_wall_clock_timeout_is_terminal() {
    let descriptor = async_descriptor(1);
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(runner_manifest("plugin-a", vec![descriptor.clone()]));
    bootstrapper.register_async_handler(Arc::new(DelayedAsyncHandler {
        descriptor,
        delay: Duration::from_secs(30),
        invocations: Arc::new(AtomicUsize::new(0)),
        active: Arc::new(AtomicUsize::new(0)),
        max_active: Arc::new(AtomicUsize::new(0)),
    }));
    let mut config = HostRuntimeConfig {
        event_driven: true,
        ..HostRuntimeConfig::default()
    };
    config.default_runner_limits.wall_clock_deadline = Some(Duration::from_millis(20));
    let config = config.with_async_executor(Arc::new(
        TokioAsyncExecutor::new(1, 1, 1, 1024 * 1024).unwrap(),
    ));
    let runtime = bootstrapper
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    let handles = submit_tasks(&runtime, 1);
    wait_for_all(&runtime, &handles, TaskStatus::Failed);
    match runtime
        .dispatch(HostRuntimeCommand::TaskOutcome(handles[0].clone()))
        .unwrap()
    {
        HostRuntimeReply::TaskOutcome(Some(TaskOutcome::Failed { error, .. })) => {
            assert_eq!(error.route, "host.async_executor.timeout")
        }
        other => panic!("unexpected task outcome reply: {other:?}"),
    }
}

#[test]
fn reload_drains_inflight_async_invocation_before_generation_swap() {
    let descriptor = async_descriptor(1);
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(runner_manifest("plugin-a", vec![descriptor.clone()]));
    bootstrapper.register_async_handler(Arc::new(GatedAsyncHandler {
        descriptor: descriptor.clone(),
        started: started.clone(),
        release: release.clone(),
    }));
    let config = HostRuntimeConfig::default().with_async_executor(Arc::new(
        TokioAsyncExecutor::new(1, 2, 2, 1024 * 1024).unwrap(),
    ));
    let mut runtime = bootstrapper
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    let handles = submit_tasks(&runtime, 1);
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    let wait_started = Instant::now();
    while started.load(Ordering::SeqCst) == 0 && wait_started.elapsed() < Duration::from_secs(1) {
        let _ = runtime.dispatch(HostRuntimeCommand::DriveState);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "task status: {:?}, executor: {:?}",
        runtime.task_status(&handles[0].task_id),
        runtime.dispatch(HostRuntimeCommand::AsyncExecutor).unwrap(),
    );

    let mut reload = RuntimeBootstrapper::new();
    reload.register_manifest(runner_manifest("plugin-a", vec![descriptor.clone()]));
    reload.register_async_handler(Arc::new(DelayedAsyncHandler {
        descriptor,
        delay: Duration::ZERO,
        invocations: Arc::new(AtomicUsize::new(0)),
        active: Arc::new(AtomicUsize::new(0)),
        max_active: Arc::new(AtomicUsize::new(0)),
    }));
    let prepared = reload.prepare_reload(runtime_profile(), 2).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let join = std::thread::spawn(move || {
        let result = runtime.reload(prepared, Duration::from_secs(2));
        done_tx.send(result.is_ok()).unwrap();
        runtime
    });

    assert!(done_rx.recv_timeout(Duration::from_millis(80)).is_err());
    release.notify_one();
    assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    let runtime = join.join().unwrap();
    assert_eq!(runtime.host_context().registry_generation(), 2);
    assert_eq!(
        runtime.task_status(&handles[0].task_id),
        Some(TaskStatus::Completed)
    );
}
