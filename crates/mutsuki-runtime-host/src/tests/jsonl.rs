use std::io::Cursor;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{Runner, RunnerContext};
use serde_json::json;

use crate::JsonlRunner;

use super::helpers::{descriptor, test_resource_ref};

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
