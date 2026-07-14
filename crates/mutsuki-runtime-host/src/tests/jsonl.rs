use std::io::Cursor;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{Runner, RunnerContext};
use serde_json::json;

use crate::JsonlRunner;

use super::helpers::{descriptor, test_resource_ref};

#[test]
fn jsonl_runner_uses_runner_run_batch_method_surface() {
    let runner_descriptor = descriptor("jsonl.runner", "raw.input");
    let mut task = Task::new("task-1", "raw.input", json!({}));
    task.lease_id = Some("task-lease-test".into());
    let batch = single_test_batch("batch-test", "task-lease-test", task);
    let result = CompletionBatch {
        batch_id: "batch-test".into(),
        tick_id: "tick-1".into(),
        results: vec![EntryCompletion {
            entry_id: "task-1".into(),
            task_id: "task-1".into(),
            result: Some(RunnerResult::completed("task-1")),
            error: None,
        }],
        metadata: Vec::new(),
    };
    let response = format!("{}\n", json!({"id":"req-1","ok":true,"result": result}));
    let reader = Cursor::new(response.into_bytes());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = JsonlRunner::new(runner_descriptor, reader, writer);

    let result = runner
        .run_batch(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("task-lease-test".into()),
                "invocation:test",
            ),
            batch,
        )
        .unwrap();
    let (_reader, writer) = runner.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert_eq!(result.batch_id, "batch-test");
    assert!(request.contains("\"method\":\"runner.run_batch\""));
    assert!(request.contains("\"batch\":"));
    assert!(!request.contains("\"task\":"));
    assert!(request.contains("\"registry_generation\":1"));
    assert!(request.contains("\"executor_id\":\"executor:test\""));
    assert!(request.contains("\"task_lease_ids\":[\"task-lease-test\"]"));
}

#[test]
fn jsonl_runner_rejects_task_lease_mismatch_before_writing_request() {
    let runner_descriptor = descriptor("jsonl.runner", "raw.input");
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = JsonlRunner::new(runner_descriptor, reader, writer);
    let mut task = Task::new("task-1", "raw.input", json!({}));
    task.lease_id = Some("task-lease-task".into());
    let batch = single_test_batch("batch-test", "task-lease-task", task);

    let error = runner
        .run_batch(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("task-lease-ctx".into()),
                "invocation:test",
            ),
            batch,
        )
        .unwrap_err();
    let (_reader, writer) = runner.into_inner();

    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);
    assert!(writer.into_inner().is_empty());
}

fn single_test_batch(batch_id: &str, lease_id: &str, task: Task) -> WorkBatch {
    let lease = TaskLease {
        lease_id: lease_id.into(),
        task_id: task.task_id.clone(),
        runner_id: "jsonl.runner".into(),
        executor_id: "executor:test".into(),
        registry_generation: 1,
        acquired_at_step: 1,
        expires_at_step: None,
    };
    WorkBatch {
        batch_id: batch_id.into(),
        tick_id: "tick-1".into(),
        batch_key: "jsonl.runner".into(),
        entries: vec![BatchEntry {
            entry_id: task.task_id.clone(),
            task_id: task.task_id.clone(),
            trace_id: task.trace_id.clone(),
            parent_id: None,
            payload_index: 0,
            resource_requirement_indices: Vec::new(),
            cancel_index: Some(0),
            deadline_tick: None,
            priority: task.priority,
            lane: DispatchLane::Normal,
            ordering: OrderingRequirement::None,
        }],
        payload: BatchPayload::from_tasks(std::slice::from_ref(&task)),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: vec![lease],
    }
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
        descriptor_updates: Vec::new(),
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
