use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{
    CoreRuntime, Runner, RunnerContext, RuntimeFailure, RuntimeResult, ScheduleDecision,
};
use serde_json::json;

use crate::{
    HostRuntimeCommand, HostRuntimeReply, JsonlRunner, NativePluginHost, NativeRunner,
    ScheduleInput, SchedulerPolicy, runner_manifest,
};

fn descriptor(id: &str, kind: &str) -> RunnerDescriptor {
    descriptor_with_class(id, kind, ExecutionClass::Cpu)
}

fn descriptor_with_class(
    id: &str,
    kind: &str,
    execution_class: ExecutionClass,
) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: id.into(),
        plugin_id: "plugin-a".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![kind.into()],
        purity: RunnerPurity::Pure,
        execution_class,
        input_schema: json!({}),
        output_schema: json!({}),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{id}")],
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
        vec!["slow-1".to_string()]
    );
}

fn runtime_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "default".into(),
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}

fn runtime_profile_with_deployment(
    plugin_id: &str,
    deployment: PluginDeploymentKind,
) -> RuntimeProfile {
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec![plugin_id.into()];
    profile
        .plugin_deployments
        .insert(plugin_id.into(), deployment);
    profile
}

fn abi_plugin_fixture() -> (PluginManifest, RunnerDescriptor) {
    let mut runner_descriptor = descriptor("abi.runner", "abi.work");
    runner_descriptor.plugin_id = "plugin-abi".into();
    let mut manifest = runner_manifest("plugin-abi", vec![runner_descriptor.clone()]);
    manifest.artifact.artifact_type = ArtifactType::Abi;
    manifest.artifact.path = "plugin-abi.so".into();
    manifest.artifact.sha256 = "sha256:abi".into();
    (manifest, runner_descriptor)
}

fn host_with_echo_runner() -> NativePluginHost {
    let runner_descriptor = descriptor("echo.runner", "raw.input");
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx: RunnerContext, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    host
}

#[test]
fn native_plugin_host_boots_runtime_and_runs_runner_loop() {
    let mut runtime: CoreRuntime = host_with_echo_runner()
        .into_runtime(runtime_profile())
        .unwrap();

    runtime.enqueue_task(Task::new("task-1", "raw.input", json!({"ok": true})));
    let report = runtime.run_until_idle(4).unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(
        runtime.tasks().get("task-1").unwrap().status,
        TaskStatus::Completed
    );
}

#[test]
fn native_plugin_host_can_boot_host_runtime_control_plane() {
    let mut runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let submitted = runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-1",
            "raw.input",
            json!({"ok": true}),
        ))))
        .unwrap();
    assert_eq!(submitted, HostRuntimeReply::TaskSubmitted("task-1".into()));

    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    let HostRuntimeReply::Idle(report) = reply else {
        panic!("expected idle reply");
    };
    assert_eq!(report.completed_tasks, 1);
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Completed));
}

#[test]
fn resolver_records_builtin_and_abi_plugin_deployments() {
    let builtin_descriptor = descriptor("builtin.runner", "builtin.work");
    let mut abi_descriptor = descriptor("abi.runner", "abi.work");
    abi_descriptor.plugin_id = "plugin-b".into();
    let builtin_manifest = runner_manifest("plugin-a", vec![builtin_descriptor]);
    let mut abi_manifest = runner_manifest("plugin-b", vec![abi_descriptor]);
    abi_manifest.artifact.artifact_type = ArtifactType::Abi;
    abi_manifest.artifact.path = "plugin-b.abi".into();
    abi_manifest.artifact.sha256 = "sha256:abi".into();
    let mut profile = runtime_profile();
    profile.enabled_plugins = vec!["plugin-a".into(), "plugin-b".into()];
    profile
        .plugin_deployments
        .insert("plugin-a".into(), PluginDeploymentKind::Builtin);
    profile
        .plugin_deployments
        .insert("plugin-b".into(), PluginDeploymentKind::Abi);

    let plan = crate::resolve_load_plan(&[builtin_manifest, abi_manifest], &profile).unwrap();

    assert_eq!(
        plan.plugin_deployments.get("plugin-a"),
        Some(&PluginDeploymentKind::Builtin)
    );
    assert_eq!(
        plan.plugin_deployments.get("plugin-b"),
        Some(&PluginDeploymentKind::Abi)
    );
}

#[test]
fn abi_plugin_boots_through_registered_abi_runner_bridge() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut host = NativePluginHost::new();
    host.register_manifest(manifest);
    host.register_abi_runner(Box::new(JsonlRunner::new(
        runner_descriptor,
        reader,
        writer,
    )));

    let runtime = host.into_runtime(runtime_profile_with_deployment(
        "plugin-abi",
        PluginDeploymentKind::Abi,
    ));

    assert!(runtime.is_ok());
}

#[test]
fn enabled_plugin_runner_requires_matching_deployment_bridge() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let profile = runtime_profile_with_deployment("plugin-abi", PluginDeploymentKind::Abi);
    let mut missing_bridge_host = NativePluginHost::new();
    missing_bridge_host.register_manifest(manifest.clone());

    let missing_bridge = missing_bridge_host
        .into_runtime(profile.clone())
        .err()
        .unwrap();

    assert_eq!(missing_bridge.error().code, ERR_RUNNER_NOT_FOUND);

    let mut mismatched_host = NativePluginHost::new();
    mismatched_host.register_manifest(manifest);
    mismatched_host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));

    let mismatched = mismatched_host.into_runtime(profile).err().unwrap();

    assert_eq!(mismatched.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn host_runtime_executes_resource_plan_commands() {
    let mut runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let HostRuntimeReply::ResourceCreated(resource) = runtime
        .dispatch(HostRuntimeCommand::CreateBlobResource {
            schema: "text.v1".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap()
    else {
        panic!("expected resource creation reply");
    };
    let export = ExportPlan {
        plan_id: "export:1".into(),
        resource,
        target: "inline_utf8".into(),
        args: json!(null),
    };

    let HostRuntimeReply::PlanReceipt(export_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteExportPlan(Box::new(export)))
        .unwrap()
    else {
        panic!("expected export receipt");
    };
    assert_eq!(export_receipt.status, "exported");
    assert_eq!(export_receipt.output, json!("hello"));

    let HostRuntimeReply::ResourceCreated(capability) = runtime
        .dispatch(HostRuntimeCommand::CreateCapabilityResource {
            kind_id: "db_pool".into(),
            schema: "db.pool.v1".into(),
        })
        .unwrap()
    else {
        panic!("expected capability creation reply");
    };
    let command = CommandPlan {
        plan_id: "command:1".into(),
        capability,
        operation: "query".into(),
        args: json!({"sql": "select 1"}),
        idempotency_key: Some("query:1".into()),
    };

    let HostRuntimeReply::PlanReceipt(command_receipt) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandPlan(Box::new(
            command.clone(),
        )))
        .unwrap()
    else {
        panic!("expected command receipt");
    };
    assert_eq!(command_receipt.status, "commanded");
    assert_eq!(command_receipt.output["operation"], "query");

    let HostRuntimeReply::PlanReceipts(batch_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteCommandBatch(Box::new(
            CommandBatch {
                batch_id: "batch:1".into(),
                commands: vec![command.clone()],
                rollback_guarantee: false,
            },
        )))
        .unwrap()
    else {
        panic!("expected batch receipts");
    };
    assert_eq!(batch_receipts.len(), 1);

    let HostRuntimeReply::PlanReceipts(saga_receipts) = runtime
        .dispatch(HostRuntimeCommand::ExecuteSagaPlan(Box::new(SagaPlan {
            saga_id: "saga:1".into(),
            steps: vec![command.clone()],
            compensations: vec![command],
        })))
        .unwrap()
    else {
        panic!("expected saga receipts");
    };
    assert_eq!(saga_receipts.len(), 1);
}

#[derive(Debug)]
struct FixedScheduler {
    limit: usize,
}

impl SchedulerPolicy for FixedScheduler {
    fn decide(&self, _input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision> {
        Ok(ScheduleDecision::new(
            "test.fixed",
            self.limit,
            "test.fixed",
        ))
    }
}

#[test]
fn custom_scheduler_can_leave_ready_task_undispatched() {
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FixedScheduler { limit: 0 });
    let mut runtime = host_with_echo_runner()
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-held",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 2 })
        .unwrap();

    let HostRuntimeReply::Idle(report) = reply else {
        panic!("expected idle reply");
    };
    assert_eq!(report.claimed_tasks, 0);
    assert_eq!(runtime.task_status("task-held"), Some(TaskStatus::Ready));
}

#[test]
fn custom_scheduler_limit_is_clamped_to_runner_capacity() {
    let runner_descriptor = descriptor("slow.runner", "slow.work");
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        move |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| {
                    if task.task_id == "slow-1" {
                        release_rx.recv().unwrap();
                    }
                    RunnerResult::completed(task.task_id)
                })
                .collect())
        },
    )));
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FixedScheduler { limit: 99 });
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    for task_id in ["slow-1", "slow-2"] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                task_id,
                "slow.work",
                json!({}),
            ))))
            .unwrap();
    }
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();

    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Running));
    assert_eq!(runtime.task_status("slow-2"), Some(TaskStatus::Ready));
    release_tx.send(()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Completed));
}

#[test]
fn custom_scheduler_cannot_dispatch_non_kernel_control_runner() {
    let runner_descriptor =
        descriptor_with_class("control.runner", "control.work", ExecutionClass::Control);
    let calls = Arc::new(Mutex::new(0usize));
    let observed = calls.clone();
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        move |_ctx, tasks| {
            *observed.lock().expect("calls mutex poisoned") += tasks.len();
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FixedScheduler { limit: 99 });
    let mut runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "control-1",
            "control.work",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 2 })
        .unwrap();

    assert_eq!(runtime.task_status("control-1"), Some(TaskStatus::Ready));
    assert_eq!(*calls.lock().expect("calls mutex poisoned"), 0);
}

#[test]
fn jsonl_runner_uses_runner_step_method_surface() {
    let runner_descriptor = descriptor("jsonl.runner", "raw.input");
    let result = vec![RunnerResult::completed("task-1")];
    let response = format!("{}\n", json!({"id":"req-1","ok":true,"result": result}));
    let reader = Cursor::new(response.into_bytes());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = JsonlRunner::new(runner_descriptor, reader, writer);
    let mut task = Task::new("task-1", "raw.input", json!({}));
    task.lease_id = Some("task-lease-test".into());

    let results = runner
        .step(
            RunnerContext {
                registry_generation: 1,
                current_step: 1,
                executor_id: "executor:test".into(),
                task_lease_id: Some("task-lease-test".into()),
            },
            vec![task],
        )
        .unwrap();
    let (_reader, writer) = runner.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert_eq!(results[0].task_id, "task-1");
    assert!(request.contains("\"method\":\"runner.step\""));
    assert!(request.contains("\"registry_generation\":1"));
    assert!(request.contains("\"executor_id\":\"executor:test\""));
    assert!(request.contains("\"task_lease_id\":\"task-lease-test\""));
}

#[test]
fn jsonl_runner_rejects_task_lease_mismatch_before_writing_request() {
    let runner_descriptor = descriptor("jsonl.runner", "raw.input");
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = JsonlRunner::new(runner_descriptor, reader, writer);
    let mut task = Task::new("task-1", "raw.input", json!({}));
    task.lease_id = Some("task-lease-task".into());

    let error = runner
        .step(
            RunnerContext {
                registry_generation: 1,
                current_step: 1,
                executor_id: "executor:test".into(),
                task_lease_id: Some("task-lease-ctx".into()),
            },
            vec![task],
        )
        .unwrap_err();
    let (_reader, writer) = runner.into_inner();

    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);
    assert!(writer.into_inner().is_empty());
}

#[test]
fn jsonl_runner_cancel_and_dispose_use_management_methods() {
    let runner_descriptor = descriptor("jsonl.runner", "raw.input");
    let response = concat!(
        "{\"id\":\"req-1\",\"ok\":true,\"result\":null}\n",
        "{\"id\":\"req-2\",\"ok\":true,\"result\":null}\n"
    );
    let reader = Cursor::new(response.as_bytes());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = JsonlRunner::new(runner_descriptor, reader, writer);

    runner.cancel("inv-1").unwrap();
    runner.dispose().unwrap();
    let (_reader, writer) = runner.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert!(request.contains("\"method\":\"runner.cancel\""));
    assert!(request.contains("\"invocation_id\":\"inv-1\""));
    assert!(request.contains("\"method\":\"runner.dispose\""));
}

#[test]
fn jsonl_runner_uses_resource_plan_method_surface() {
    let runner_descriptor = descriptor("jsonl.runner", "raw.input");
    let resource = test_resource_ref("resource:text", "text", ResourceSemantic::FrozenValue);
    let capability = test_resource_ref(
        "resource:db",
        "db_pool",
        ResourceSemantic::CapabilityResource,
    );
    let export = ExportPlan {
        plan_id: "export:1".into(),
        resource: resource.clone(),
        target: "inline_utf8".into(),
        args: json!(null),
    };
    let command = CommandPlan {
        plan_id: "command:1".into(),
        capability: capability.clone(),
        operation: "query".into(),
        args: json!({"sql": "select 1"}),
        idempotency_key: Some("query:1".into()),
    };
    let receipt = PlanReceipt {
        plan_id: "receipt:1".into(),
        status: "commanded".into(),
        resource_ref: Some(capability),
        snapshot: None,
        new_version: None,
        output: json!({"ok": true}),
    };
    let response = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"id": "req-1", "ok": true, "result": receipt.clone()}),
        json!({"id": "req-2", "ok": true, "result": receipt.clone()}),
        json!({"id": "req-3", "ok": true, "result": [receipt.clone()]}),
        json!({"id": "req-4", "ok": true, "result": [receipt]}),
    );
    let reader = Cursor::new(response.into_bytes());
    let writer = Cursor::new(Vec::<u8>::new());
    let runner = JsonlRunner::new(runner_descriptor, reader, writer);

    assert_eq!(
        runner.execute_export_plan(&export).unwrap().status,
        "commanded"
    );
    assert_eq!(
        runner.execute_command_plan(&command).unwrap().status,
        "commanded"
    );
    assert_eq!(
        runner
            .execute_command_batch(&CommandBatch {
                batch_id: "batch:1".into(),
                commands: vec![command.clone()],
                rollback_guarantee: false,
            })
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        runner
            .execute_saga_plan(&SagaPlan {
                saga_id: "saga:1".into(),
                steps: vec![command.clone()],
                compensations: vec![command],
            })
            .unwrap()
            .len(),
        1
    );
    let (_reader, writer) = runner.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert!(request.contains("\"method\":\"resource.export\""));
    assert!(request.contains("\"method\":\"resource.command\""));
    assert!(request.contains("\"method\":\"resource.command_batch\""));
    assert!(request.contains("\"method\":\"resource.saga\""));
    assert!(request.contains("\"target\":\"inline_utf8\""));
}

#[test]
fn resolver_emits_declared_runtime_surfaces() {
    let runner_descriptor = descriptor("echo.runner", "raw.input");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.provides.protocols = vec![ProtocolDescriptor {
        protocol_id: "im.message.received.v1".into(),
        version: "1.0.0".into(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        error_schema: json!({"type": "object"}),
        codec: "json".into(),
        compatibility: "semver".into(),
    }];
    manifest.provides.handler_bindings = vec![HandlerBinding {
        binding_id: "message-handler".into(),
        plugin_id: "plugin-a".into(),
        protocol_id: "im.message.received.v1".into(),
        target_protocol_id: "raw.input".into(),
        target_runner_hint: Some("echo.runner".into()),
        pool_id: "default".into(),
        priority: 1,
        policy: "required".into(),
        metadata: BTreeMap::new(),
    }];
    manifest.provides.resource_schemas = vec!["bytes.v1".into()];
    manifest.provides.resource_providers = vec!["resource.local".into()];
    manifest.provides.effects = vec!["effect.chat.send".into()];
    manifest.provides.streams = vec!["chat.events".into()];
    manifest.provides.subscriptions = vec!["chat.messages".into()];
    manifest.provides.timers = vec!["heartbeat".into()];
    manifest.provides.state_schemas = vec!["state.actor.v1".into()];
    let profile = RuntimeProfile {
        profile_id: "default".into(),
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    };

    let plan = crate::resolve_load_plan(&[manifest], &profile).unwrap();

    assert_surface(
        &plan,
        "protocol:im.message.received.v1",
        ContractSurfaceKind::Protocol,
    );
    assert_surface(
        &plan,
        "handler_binding:message-handler",
        ContractSurfaceKind::HandlerBinding,
    );
    assert_surface(
        &plan,
        "resource_schema:bytes.v1",
        ContractSurfaceKind::ResourceSchema,
    );
    assert_surface(
        &plan,
        "resource_provider:resource.local",
        ContractSurfaceKind::ResourceProvider,
    );
    assert_surface(
        &plan,
        "effect:effect.chat.send",
        ContractSurfaceKind::Effect,
    );
    assert_surface(&plan, "stream:chat.events", ContractSurfaceKind::Stream);
    assert_surface(
        &plan,
        "subscription:chat.messages",
        ContractSurfaceKind::Subscription,
    );
    assert_surface(&plan, "timer:heartbeat", ContractSurfaceKind::Timer);
    assert_surface(
        &plan,
        "state_schema:state.actor.v1",
        ContractSurfaceKind::StateSchema,
    );
}

fn assert_surface(plan: &RuntimeLoadPlan, surface_id: &str, kind: ContractSurfaceKind) {
    assert!(
        plan.contract_surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.kind == kind),
        "missing surface {surface_id}"
    );
}

fn test_resource_ref(ref_id: &str, kind_id: &str, semantic: ResourceSemantic) -> ResourceRef {
    ResourceRef {
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version: 1,
        },
        ref_id: ref_id.into(),
        semantic,
        provider_id: "mutsuki.host.test".into(),
        resource_kind: kind_id.into(),
        schema: format!("{kind_id}.v1"),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: None,
        content_hash: None,
        lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}
