use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure};
use serde_json::json;

use crate::{
    HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, NativePluginHost, NativeRunner,
    RunnerLimits, runner_manifest,
};

use super::helpers::{descriptor, descriptor_with_class, runtime_profile};

#[test]
fn host_actor_accepts_work_while_blocking_runner_is_stuck() {
    let blocking_descriptor =
        descriptor_with_class("blocking.runner", "blocking.work", ExecutionClass::Blocking);
    let echo_descriptor = descriptor("echo.runner", "raw.input");
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest(
        "plugin-a",
        vec![blocking_descriptor.clone(), echo_descriptor.clone()],
    ));
    host.register_runner(Box::new(NativeRunner::new(
        blocking_descriptor,
        move |_ctx, tasks| {
            release_rx.recv().unwrap();
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    host.register_runner(Box::new(NativeRunner::new(
        echo_descriptor,
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let mut runtime = host.into_host_runtime(runtime_profile()).unwrap();

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
fn host_runtime_routes_execution_classes_to_named_worker_pools() {
    let descriptor = descriptor_with_class("script.runner", "script.work", ExecutionClass::Script);
    let observed_thread = Arc::new(Mutex::new(String::new()));
    let observed = observed_thread.clone();
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest("plugin-a", vec![descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        descriptor,
        move |_ctx, tasks| {
            *observed.lock().expect("observed thread mutex poisoned") = std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string();
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let mut runtime = host.into_host_runtime(runtime_profile()).unwrap();

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
    let mut host = NativePluginHost::new();
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
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let mut runtime = host.into_host_runtime(runtime_profile()).unwrap();

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

        fn step(
            &mut self,
            _ctx: RunnerContext,
            tasks: Vec<Task>,
        ) -> mutsuki_runtime_core::RuntimeResult<Vec<RunnerResult>> {
            self.started_tx.send(()).unwrap();
            self.release_rx.recv().unwrap();
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
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
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(CancellableRunner {
        descriptor: runner_descriptor,
        started_tx,
        release_rx,
        cancelled: cancelled.clone(),
    }));
    let mut runtime = host.into_host_runtime(runtime_profile()).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "slow-1",
            "slow.work",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Running));

    assert_eq!(
        runtime
            .dispatch(HostRuntimeCommand::CancelTask("slow-1".into()))
            .unwrap(),
        HostRuntimeReply::TaskCancelled("slow-1".into())
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
        vec!["task-lease-1-slow-1".to_string()]
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

        fn step(
            &mut self,
            ctx: RunnerContext,
            tasks: Vec<Task>,
        ) -> mutsuki_runtime_core::RuntimeResult<Vec<RunnerResult>> {
            assert_eq!(ctx.deadline_tick, Some(2));
            assert_eq!(ctx.invocation_id, "task-lease-1-deadline-1");
            self.started_tx.send(()).unwrap();
            self.release_rx.recv().unwrap();
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
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
    let mut host = NativePluginHost::new();
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
    let mut runtime = host
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
        vec!["task-lease-1-deadline-1".to_string()]
    );
}
