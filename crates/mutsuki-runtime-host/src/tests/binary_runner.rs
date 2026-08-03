use std::io::Cursor;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{Runner, RunnerContext};
use mutsuki_runtime_wire::{
    BINARY_CODEC_ID, CommandBatchRequest, CommandPlanRequest, DEFAULT_WIRE_LIMITS,
    ExportPlanRequest, Opcode, ProtocolHello, ProtocolHelloAck, RunBatchRequest, SagaPlanRequest,
    decode_binary_frame, decode_binary_request, encode_binary_response,
};
use serde::Serialize;
use serde_json::json;

use crate::BinaryRunner;

use super::helpers::{descriptor, test_resource_ref};

#[test]
fn binary_runner_uses_runner_run_batch_method_surface() {
    let runner_descriptor = descriptor("binary.runner", "raw.input");
    let mut task = Task::new("task-1", "raw.input", json!({}));
    task.lease_id = Some("task-lease-test".into());
    let batch = single_test_batch("batch-test", "task-lease-test", task);
    let completion = CompletionBatch {
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
    let response = typed_responses(&[(Opcode::RunnerRunBatch, &completion)]);
    let reader = Cursor::new(response);
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = BinaryRunner::new(runner_descriptor, reader, writer);

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
    let bytes = writer.into_inner();
    let frames = split_frames(&bytes);
    let (_, request) =
        decode_binary_request::<RunBatchRequest>(frames[1], DEFAULT_WIRE_LIMITS).unwrap();

    assert_eq!(result.batch_id, "batch-test");
    assert_eq!(request.runner_id, "binary.runner");
    assert_eq!(request.batch.batch_id, "batch-test");
    assert_eq!(request.batch.row_payload_tasks().unwrap().len(), 1);
    assert_eq!(request.ctx.registry_generation, 1);
    assert_eq!(request.ctx.executor_id, "executor:test");
    assert_eq!(request.ctx.task_lease_ids, vec!["task-lease-test"]);
}

#[test]
fn binary_runner_rejects_task_lease_mismatch_before_writing_request() {
    let runner_descriptor = descriptor("binary.runner", "raw.input");
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = BinaryRunner::new(runner_descriptor, reader, writer);
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
        runner_id: "binary.runner".into(),
        attempt_generation: 1,
        executor_id: "executor:test".into(),
        registry_generation: 1,
        acquired_at_step: 1,
        expires_at_step: None,
    };
    WorkBatch {
        batch_id: batch_id.into(),
        tick_id: "tick-1".into(),
        batch_key: "binary.runner".into(),
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
fn binary_runner_cancel_and_dispose_use_management_methods() {
    let runner_descriptor = descriptor("binary.runner", "raw.input");
    let response = typed_responses(&[(Opcode::RunnerCancel, &()), (Opcode::RunnerDispose, &())]);
    let reader = Cursor::new(response);
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = BinaryRunner::new(runner_descriptor, reader, writer);

    runner.cancel("inv-1").unwrap();
    runner.dispose().unwrap();
    let (_reader, writer) = runner.into_inner();
    let bytes = writer.into_inner();
    let frames = split_frames(&bytes);
    let cancel = decode_binary_frame(frames[1], DEFAULT_WIRE_LIMITS).unwrap();
    let dispose = decode_binary_frame(frames[2], DEFAULT_WIRE_LIMITS).unwrap();

    assert_eq!(cancel.header.opcode, Opcode::RunnerCancel);
    assert!(cancel.header.opcode.is_management());
    assert_eq!(dispose.header.opcode, Opcode::RunnerDispose);
    assert!(dispose.header.opcode.is_management());
}

#[test]
fn binary_runner_uses_resource_plan_method_surface() {
    let runner_descriptor = descriptor("binary.runner", "raw.input");
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
    let receipt_batch = vec![receipt.clone()];
    let receipt_saga = vec![receipt.clone()];
    let response = typed_response_bytes(&[
        (
            Opcode::ResourceExport,
            serde_json::to_value(&receipt).unwrap(),
        ),
        (
            Opcode::ResourceCommand,
            serde_json::to_value(&receipt).unwrap(),
        ),
        (
            Opcode::ResourceCommandBatch,
            serde_json::to_value(&receipt_batch).unwrap(),
        ),
        (
            Opcode::ResourceSaga,
            serde_json::to_value(&receipt_saga).unwrap(),
        ),
    ]);
    let reader = Cursor::new(response);
    let writer = Cursor::new(Vec::<u8>::new());
    let runner = BinaryRunner::new(runner_descriptor, reader, writer);

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
    let bytes = writer.into_inner();
    let frames = split_frames(&bytes);
    let (_, export_request) =
        decode_binary_request::<ExportPlanRequest>(frames[1], DEFAULT_WIRE_LIMITS).unwrap();
    let (_, command_request) =
        decode_binary_request::<CommandPlanRequest>(frames[2], DEFAULT_WIRE_LIMITS).unwrap();
    let (_, batch_request) =
        decode_binary_request::<CommandBatchRequest>(frames[3], DEFAULT_WIRE_LIMITS).unwrap();
    let (_, saga_request) =
        decode_binary_request::<SagaPlanRequest>(frames[4], DEFAULT_WIRE_LIMITS).unwrap();

    assert_eq!(export_request.plan.target, "inline_utf8");
    assert_eq!(command_request.plan.operation, "query");
    assert_eq!(batch_request.batch.batch_id, "batch:1");
    assert_eq!(saga_request.saga.saga_id, "saga:1");
}

fn typed_responses<T: Serialize>(responses: &[(Opcode, &T)]) -> Vec<u8> {
    typed_response_bytes(
        &responses
            .iter()
            .map(|(opcode, value)| (*opcode, serde_json::to_value(value).unwrap()))
            .collect::<Vec<_>>(),
    )
}

fn typed_response_bytes(responses: &[(Opcode, serde_json::Value)]) -> Vec<u8> {
    let hello = ProtocolHello::binary();
    let ack: ProtocolHelloAck = hello.accept(BINARY_CODEC_ID, None).unwrap();
    let mut encoded =
        encode_binary_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS).unwrap();
    for (index, (opcode, value)) in responses.iter().enumerate() {
        encoded.extend(
            encode_binary_response(index as u64 + 2, *opcode, Ok(value), DEFAULT_WIRE_LIMITS)
                .unwrap(),
        );
    }
    encoded
}

fn split_frames(bytes: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset + 4 + len;
        frames.push(&bytes[offset..end]);
        offset = end;
    }
    frames
}
