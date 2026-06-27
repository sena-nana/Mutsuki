use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, mpsc};

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{CoreRuntime, Runner, RunnerContext, RuntimeFailure};
use serde_json::json;

use crate::{
    HostRuntimeCommand, HostRuntimeReply, JsonlRunner, NativePluginHost, NativeRunner,
    runner_manifest,
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

fn runtime_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "default".into(),
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
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
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    };

    let plan = crate::resolve_load_plan(&[manifest], &profile);

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
