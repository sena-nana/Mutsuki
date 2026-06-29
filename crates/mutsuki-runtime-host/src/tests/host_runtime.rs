use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure};
use mutsuki_runtime_sdk::HostRuntime as SdkHostRuntime;
use serde_json::json;

use crate::{
    HostRuntime, HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, NativePluginHost,
    NativeRunner, RunnerLimits, runner_manifest,
};

use super::helpers::{descriptor, descriptor_with_class, runtime_profile};

struct BlockingObservedRunner {
    descriptor: RunnerDescriptor,
    started_tx: mpsc::Sender<()>,
    release_rx: mpsc::Receiver<()>,
    cancelled: Arc<Mutex<Vec<String>>>,
    disposed: Arc<Mutex<bool>>,
}

impl Runner for BlockingObservedRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(
        &mut self,
        _ctx: RunnerContext,
        tasks: Vec<Task>,
    ) -> mutsuki_runtime_core::RuntimeResult<Vec<RunnerResult>> {
        self.started_tx.send(()).unwrap();
        let _ = self.release_rx.recv();
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

    fn dispose(&mut self) -> mutsuki_runtime_core::RuntimeResult<()> {
        *self.disposed.lock().expect("disposed mutex poisoned") = true;
        Ok(())
    }
}

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
    let mut host = NativePluginHost::new();
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
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
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
        vec!["task-lease-1-wall-stuck-1".to_string()]
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
    let mut host = NativePluginHost::new();
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
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let config = HostRuntimeConfig {
        blocking_threads: 1,
        cancel_grace_period: Some(Duration::from_millis(30)),
        ..HostRuntimeConfig::default()
    };
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "cancel-stuck-1",
            "cancel.stuck",
            json!({}),
        ))))
        .unwrap();
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::CancelTask("cancel-stuck-1".into()))
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
        vec!["task-lease-1-cancel-stuck-1".to_string()]
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
    let mut host = NativePluginHost::new();
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
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
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
        vec!["task-lease-1-health-stuck-1".to_string()]
    );
    assert!(*disposed.lock().expect("disposed mutex poisoned"));
}

#[test]
fn host_runtime_registers_only_active_capability_graph_extensions() {
    let runner_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor.clone()]);
    manifest.requires = vec![
        "scheduler_policy:scheduler.fair".into(),
        "workflow:workflow.linear".into(),
    ];
    manifest.provides.host_backends = vec![
        HostBackendDescriptor {
            backend_id: "host.backend.builtin".into(),
            kind: HostExtensionKind::PluginBackend,
            supported_deployments: vec![PluginDeploymentKind::Builtin],
            reload_policy: "static".into(),
            drain_required: false,
        },
        HostBackendDescriptor {
            backend_id: "host.backend.abi".into(),
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
    let mut host = NativePluginHost::new();
    host.register_manifest(manifest);
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let mut profile = runtime_profile();
    profile.mode = RuntimeProfileMode::LockedBuiltin;
    profile.allow_hot_reload = false;
    let runtime = host.into_host_runtime(profile).unwrap();

    assert!(
        runtime
            .capabilities()
            .require_host_backend("host.backend.builtin")
            .is_ok()
    );
    assert!(
        runtime
            .host_context()
            .capability_broker()
            .require_host_backend("host.backend.builtin")
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
            .require_host_backend("host.backend.abi"),
        "host_backend:host.backend.abi",
    );
}

#[test]
fn host_runtime_sdk_context_submits_tasks_and_requests_shutdown() {
    let mut runtime = super::helpers::host_with_echo_runner()
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
        SdkHostRuntime::task_outcome(&runtime, "sdk-host-task").unwrap(),
        Some(TaskOutcome::Completed { task_id, .. }) if task_id == "sdk-host-task"
    ));
    SdkHostRuntime::request_shutdown(&runtime, "test.shutdown").unwrap();
    assert!(runtime.host_context().shutdown().is_shutdown_requested());
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
