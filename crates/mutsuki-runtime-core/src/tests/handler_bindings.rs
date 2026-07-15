use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;
fn handler_binding(
    binding_id: &str,
    protocol_id: &str,
    target_protocol_id: &str,
) -> HandlerBinding {
    HandlerBinding {
        binding_id: binding_id.into(),
        plugin_id: "plugin-a".into(),
        protocol_id: protocol_id.into(),
        target_protocol_id: target_protocol_id.into(),
        target_runner_hint: None,
        pool_id: "default".into(),
        priority: 0,
        policy: "required".into(),
        metadata: BTreeMap::new(),
    }
}

fn boot_error(plan: RuntimeLoadPlan, runners: Vec<Box<dyn Runner>>) -> RuntimeFailure {
    match CoreRuntime::boot(plan, runners) {
        Ok(_) => panic!("runtime boot should fail"),
        Err(error) => error,
    }
}

#[test]
fn handler_bindings_are_queryable_but_do_not_fan_out_tasks() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("message.runner".into());
    binding.priority = 10;
    let plan = load_plan(vec![runner.clone()], vec![binding.clone()]);
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    assert_eq!(
        runtime.handler_bindings("im.message.received.v1"),
        vec![&binding]
    );

    runtime
        .submit_task(Task::new(
            "raw-1",
            "im.message.received.v1",
            json!({"text": "hello"}),
        ))
        .unwrap();
    let report = runtime.tick_once().unwrap();

    assert_eq!(report.claimed_tasks, 0);
    assert!(runtime.tasks().get("raw-1").is_some());
    assert_eq!(runtime.tasks().get("raw-1:message-handler"), None);
}

#[test]
fn core_facade_can_submit_one_targeted_task_from_handler_binding() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("message.runner".into());
    let plan = load_plan(vec![runner.clone()], vec![binding]);
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(runner, |task| {
        assert_eq!(task.target_binding_id.as_deref(), Some("message-handler"));
        assert_eq!(task.protocol_id, "cap.message.handle");
        RunnerResult::completed(task.task_id.clone())
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime
        .submit_targeted_task("targeted-1", "message-handler", json!({"text": "hello"}))
        .unwrap();
    let report = runtime.tick_once().unwrap();

    assert_eq!(report.claimed_tasks, 1);
    assert_eq!(
        runtime.task_status("targeted-1"),
        Some(TaskStatus::Completed)
    );
}

#[test]
fn handler_binding_register_facade_is_closed_after_boot() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    let plan = load_plan(vec![runner.clone()], vec![binding.clone()]);
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    let err = runtime.register_handler_binding(binding).unwrap_err();

    assert_eq!(err.error().code, ERR_REGISTRY_FROZEN);
}

#[test]
fn runtime_boot_rejects_non_kernel_committer_runner() {
    let committer = runner_descriptor("plugin.committer", "core.commit", RunnerPurity::Committer);
    let plan = load_plan(vec![committer.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(committer));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn runtime_boot_rejects_non_kernel_control_runner() {
    let mut control = runner_descriptor("plugin.control", "control.work", RunnerPurity::Pure);
    control.execution_class = ExecutionClass::Control;
    let plan = load_plan(vec![control.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(control));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn runtime_boot_rejects_incoherent_runner_batch_capability() {
    let mut runner = runner_descriptor("batch.worker", "cap.work", RunnerPurity::Pure);
    runner.payload.preferred_layout = PayloadLayout::BinaryPacked;
    runner.payload.layouts = vec![PayloadLayout::Row];
    let plan = load_plan(vec![runner.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(err.error().route, "runner.batch.worker.payload");
}

#[test]
fn runtime_boot_rejects_entry_concurrency_above_batch_limit() {
    let mut runner = runner_descriptor("batch.worker", "cap.work", RunnerPurity::Pure);
    runner.batch.max_batch_entries = 2;
    runner.batch.max_entry_concurrency = 3;
    let plan = load_plan(vec![runner.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(err.error().route, "runner.batch.worker.batch");
}

#[test]
fn runtime_boot_rejects_multiple_inflight_batches_for_single_instance_runner() {
    let mut runner = runner_descriptor("batch.worker", "cap.work", RunnerPurity::Pure);
    runner.batch.max_inflight_batches = 2;
    let plan = load_plan(vec![runner.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(err.error().route, "runner.batch.worker.batch");
}

#[test]
fn runner_control_facade_respects_freeze_and_authorized_capabilities() {
    let runner = runner_descriptor("worker", "cap.work", RunnerPurity::Pure);
    let plan = load_plan(vec![runner.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner.clone()));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    let heartbeat = runtime
        .runner_heartbeat("worker", "executor-worker-1")
        .unwrap();
    assert_eq!(heartbeat.executor_id, "executor-worker-1");
    let declaration = runtime
        .runner_capability("worker", vec!["cap.work".into()], 2)
        .unwrap();
    assert_eq!(declaration.capacity, 2);
    assert_eq!(
        runtime
            .runner_capability("worker", vec!["cap.other".into()], 1)
            .unwrap_err()
            .error()
            .code,
        ERR_REGISTRY_UNAUTHORIZED
    );

    let register_err = runtime
        .register_runner(completed_runner!(runner.clone()))
        .unwrap_err();
    assert_eq!(register_err.error().code, ERR_REGISTRY_FROZEN);
    let unregister_err = runtime.unregister_runner("worker").unwrap_err();
    assert_eq!(unregister_err.error().code, ERR_REGISTRY_FROZEN);
}

#[test]
fn dispose_plugins_calls_registered_runner_management_surface() {
    let runner = runner_descriptor("worker", "cap.work", RunnerPurity::Pure);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let plan = load_plan(vec![runner.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> =
        vec![Box::new(ContinuingRunner::new(runner, calls.clone()))];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    let disposed = runtime.dispose_plugins().unwrap();

    assert_eq!(disposed.disposed, vec!["worker".to_string()]);
    assert_eq!(
        *calls.lock().expect("calls mutex poisoned"),
        vec!["dispose:worker".to_string()]
    );
}

#[test]
fn handler_binding_load_plan_validation_rejects_missing_protocol() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let plan = load_plan(
        vec![runner.clone()],
        vec![handler_binding(
            "message-handler",
            "im.message.received.v1",
            "cap.message.missing",
        )],
    );
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn handler_binding_load_plan_validation_rejects_bad_runner_hint() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("missing.runner".into());
    let plan = load_plan(vec![runner.clone()], vec![binding]);
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(runner));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn handler_binding_load_plan_validation_rejects_hint_that_cannot_handle_protocol() {
    let hinted = runner_descriptor("hinted.runner", "cap.other", RunnerPurity::Pure);
    let target = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("hinted.runner".into());
    let plan = load_plan(vec![hinted.clone(), target.clone()], vec![binding]);
    let runners: Vec<Box<dyn Runner>> =
        runners_with_kernel!(completed_runner!(hinted), completed_runner!(target));

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}
