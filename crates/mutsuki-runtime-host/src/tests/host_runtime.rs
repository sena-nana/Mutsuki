#![allow(clippy::field_reassign_with_default)]

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{
    Runner, RunnerContext, RuntimeFailure, RuntimeResult, RuntimeStopState, ScheduleDecision,
};
use mutsuki_runtime_sdk::HostRuntime as SdkHostRuntime;
use serde_json::json;

use crate::{
    HostRuntime, HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, NativeRunner,
    RunnerLimits, RuntimeBootstrapper, ScheduleInput, SchedulerPolicy, runner_manifest,
};

use super::helpers::{descriptor, descriptor_with_class, runtime_profile};

struct BlockingObservedRunner {
    descriptor: RunnerDescriptor,
    started_tx: mpsc::Sender<()>,
    release_rx: mpsc::Receiver<()>,
    cancelled: Arc<Mutex<Vec<String>>>,
    disposed: Arc<Mutex<bool>>,
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
    assert_eq!(snapshots[0].trace_id.as_deref(), Some("trace-1"));
    assert_eq!(snapshots[0].input_refs, vec!["input:1".to_string()]);
    assert_eq!(
        snapshots[0].required_surfaces,
        vec!["surface:snapshot".to_string()]
    );
    assert_eq!(
        snapshots[0].claimed_by.as_deref(),
        Some("snapshot.blocking.runner")
    );
    assert_eq!(
        snapshots[0].owner_runner.as_deref(),
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

    let mut runtime = super::helpers::host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    let mut changed_descriptor = descriptor("echo.runner", "raw.input");
    changed_descriptor.input_schema = json!({"changed": true});
    let disposed = Arc::new(Mutex::new(false));
    let mut reload_host = RuntimeBootstrapper::new();
    reload_host.register_manifest(runner_manifest(
        "plugin-a",
        vec![changed_descriptor.clone()],
    ));
    reload_host.register_runner(Box::new(ObservedDisposeRunner {
        descriptor: changed_descriptor,
        disposed: disposed.clone(),
    }));
    let mut prepared = reload_host.prepare_reload(runtime_profile(), 2).unwrap();
    prepared.plan.contract_surfaces[0].fingerprint = "sha256:breaking".into();

    assert!(runtime.reload(prepared, Duration::from_secs(1)).is_err());
    assert_eq!(runtime.host_context().registry_generation(), 1);
    assert!(*disposed.lock().expect("disposed mutex poisoned"));
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
            .contains("script-worker")
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
fn cancel_running_task_is_delivered_when_worker_returns_runner() {
    struct CancellableRunner {
        descriptor: RunnerDescriptor,
        started_tx: mpsc::Sender<()>,
        release_rx: mpsc::Receiver<()>,
        cancelled: Arc<Mutex<Vec<String>>>,
    }

    impl Runner for CancellableRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.descriptor
        }

        fn run_batch(
            &mut self,
            _ctx: RunnerContext,
            batch: WorkBatch,
        ) -> mutsuki_runtime_core::RuntimeResult<CompletionBatch> {
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
        descriptor_with_class("cancellable.runner", "slow.work", ExecutionClass::Blocking);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(CancellableRunner {
        descriptor: runner_descriptor,
        started_tx,
        release_rx,
        cancelled: cancelled.clone(),
    }));
    let runtime = host.into_host_runtime(runtime_profile()).unwrap();

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

    assert_eq!(
        runtime
            .dispatch(HostRuntimeCommand::CancelTask(slow_handle.clone()))
            .unwrap(),
        HostRuntimeReply::TaskCancelled(slow_handle)
    );
    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Cancelled));
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

    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Cancelled));
    assert_eq!(
        *cancelled.lock().expect("cancelled mutex poisoned"),
        vec!["batch-1-cancellable.runner-1".to_string()]
    );
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
    assert_eq!(
        runtime.task_status("deadline-1"),
        Some(TaskStatus::Cancelled)
    );
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
        Some(TaskStatus::Cancelled)
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
    wait_for_status(&mut runtime, "wall-echo-1", TaskStatus::Completed);
    assert_eq!(
        runtime.task_status("wall-echo-1"),
        Some(TaskStatus::Completed)
    );

    release_tx.send(()).unwrap();
    wait_for_dispose(&mut runtime, &disposed);
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
        Some(TaskStatus::Cancelled)
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
    wait_for_status(&mut runtime, "cancel-echo-1", TaskStatus::Completed);
    assert_eq!(
        runtime.task_status("cancel-echo-1"),
        Some(TaskStatus::Completed)
    );

    release_tx.send(()).unwrap();
    wait_for_dispose(&mut runtime, &disposed);
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
        Some(TaskStatus::Cancelled)
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
    wait_for_status(&mut runtime, "health-echo-1", TaskStatus::Completed);
    assert_eq!(
        runtime.task_status("health-echo-1"),
        Some(TaskStatus::Completed)
    );

    release_tx.send(()).unwrap();
    wait_for_dispose(&mut runtime, &disposed);
    assert_eq!(
        *cancelled.lock().expect("cancelled mutex poisoned"),
        vec!["batch-1-stuck.health.runner-1".to_string()]
    );
    assert!(*disposed.lock().expect("disposed mutex poisoned"));
}

#[test]
fn host_runtime_requires_active_scheduler_policy_instance() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor.clone()]);
    manifest.requires = vec!["scheduler_policy:scheduler.fair".into()];
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
        "scheduler_policy:scheduler.fair".into(),
        "workflow:workflow.linear".into(),
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
            codec_id: Some("codec.json".into()),
            bridge_id: Some("bridge.abi.jsonl".into()),
        },
    ];
    manifest.provides.codecs = vec![CodecDescriptor {
        codec_id: "codec.json".into(),
        media_type: "application/json".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    }];
    manifest.provides.bridges = vec![BridgeDescriptor {
        bridge_id: "bridge.abi.jsonl".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        codec_ids: vec!["codec.json".into()],
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
        runtime.capabilities().require_bridge("bridge.abi.jsonl"),
        "bridge:bridge.abi.jsonl",
    );
    assert_pruned_capability(
        runtime.capabilities().require_codec("codec.json"),
        "codec:codec.json",
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
