use std::collections::BTreeMap;

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;
fn handler_binding(binding_id: &str, protocol_id: &str, target_task_kind: &str) -> HandlerBinding {
    HandlerBinding {
        binding_id: binding_id.into(),
        plugin_id: "plugin-a".into(),
        protocol_id: protocol_id.into(),
        target_task_kind: target_task_kind.into(),
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
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(runner, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    assert_eq!(
        runtime.handler_bindings("im.message.received.v1"),
        vec![&binding]
    );

    runtime.publish_raw_input("raw-1", "im.message.received.v1", json!({"text": "hello"}));
    let report = runtime.tick_once().unwrap();

    assert_eq!(report.claimed_tasks, 0);
    assert!(runtime.tasks().get("raw-1").is_some());
    assert_eq!(runtime.tasks().get("raw-1:message-handler"), None);
}

#[test]
fn handler_binding_load_plan_validation_rejects_missing_task_kind() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let plan = load_plan(
        vec![runner.clone()],
        vec![handler_binding(
            "message-handler",
            "im.message.received.v1",
            "cap.message.missing",
        )],
    );
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(runner, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];

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
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(runner, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn handler_binding_load_plan_validation_rejects_hint_that_cannot_handle_kind() {
    let hinted = runner_descriptor("hinted.runner", "cap.other", RunnerPurity::Pure);
    let target = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("hinted.runner".into());
    let plan = load_plan(vec![hinted.clone(), target.clone()], vec![binding]);
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(hinted, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(StaticRunner::new(target, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}
