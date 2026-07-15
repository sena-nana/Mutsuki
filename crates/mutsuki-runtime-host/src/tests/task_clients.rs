use std::io::Cursor;
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_wire::{
    DEFAULT_WIRE_LIMITS, Opcode, ProtocolHello, ProtocolHelloAck, encode_jsonl_response,
};
use serde_json::json;

use crate::{AbiTaskClient, LocalTaskClient, TaskClient};
use mutsuki_runtime_sdk::TaskBatchBuilder;

use super::helpers::{host_with_echo_runner, runtime_profile};

#[test]
fn host_task_clients_share_task_contract_across_local_and_abi_backends() {
    let runtime = Arc::new(Mutex::new(
        host_with_echo_runner()
            .into_runtime(runtime_profile())
            .unwrap(),
    ));
    let local = LocalTaskClient::new(runtime);
    let mut local_task = Task::new("local-client-task", "raw.input", json!({"source": "local"}));
    local_task.trace_id = Some("trace-local".into());
    local_task.correlation_id = Some("corr-local".into());

    let local_handle = local.submit_task(local_task).unwrap();

    assert_eq!(local_handle.task_id, "local-client-task");
    assert_eq!(local_handle.protocol_id, "raw.input");
    assert_eq!(local_handle.trace_id.as_deref(), Some("trace-local"));
    assert_eq!(local_handle.correlation_id.as_deref(), Some("corr-local"));

    local.cancel_task(&local_handle).unwrap();
    assert!(matches!(
        local.task_outcome(&local_handle).unwrap(),
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "local-client-task"
    ));

    let mut abi_task = Task::new("abi-client-task", "raw.input", json!({"source": "abi"}));
    abi_task.trace_id = Some("trace-abi".into());
    abi_task.correlation_id = Some("corr-abi".into());
    let abi_handle = TaskHandle {
        task_id: abi_task.task_id.clone(),
        protocol_id: abi_task.protocol_id.clone(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: abi_task.trace_id.clone(),
        correlation_id: abi_task.correlation_id.clone(),
    };
    let abi_outcome = TaskOutcome::Cancelled {
        task_id: abi_task.task_id.clone(),
        reason: Some("test.cancel".into()),
    };
    let response = typed_response_bytes(&[
        (
            Opcode::TaskSubmitBatch,
            serde_json::to_value([abi_handle]).unwrap(),
        ),
        (Opcode::TaskCancel, serde_json::Value::Null),
        (
            Opcode::TaskOutcome,
            serde_json::to_value(abi_outcome).unwrap(),
        ),
    ]);
    let abi = AbiTaskClient::new(Cursor::new(response), Cursor::new(Vec::<u8>::new()));

    let submitted = abi.submit_one(abi_task).unwrap();
    abi.cancel_task(&submitted).unwrap();
    let outcome = abi.task_outcome(&submitted).unwrap();
    let (_reader, writer) = abi.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert_eq!(submitted.task_id, "abi-client-task");
    assert_eq!(submitted.trace_id.as_deref(), Some("trace-abi"));
    assert!(matches!(
        outcome,
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "abi-client-task"
    ));
    assert!(request.contains("\"method\":\"task.submit_batch\""));
    assert!(request.contains("\"method\":\"task.cancel\""));
    assert!(request.contains("\"method\":\"task.outcome\""));
    assert!(request.contains("\"trace_id\":\"trace-abi\""));
}

#[test]
fn task_clients_implement_sdk_task_submitter_boundary() {
    let runtime = Arc::new(Mutex::new(
        host_with_echo_runner()
            .into_runtime(runtime_profile())
            .unwrap(),
    ));
    let local = LocalTaskClient::new(runtime);
    let local_handle = mutsuki_runtime_sdk::TaskSubmitter::submit_task(
        &local,
        Task::new("sdk-local-task", "raw.input", json!({})),
    )
    .unwrap();
    mutsuki_runtime_sdk::TaskSubmitter::cancel_task(&local, &local_handle).unwrap();

    assert_eq!(local_handle.task_id, "sdk-local-task");
    assert!(matches!(
        mutsuki_runtime_sdk::TaskSubmitter::task_outcome(&local, &local_handle).unwrap(),
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "sdk-local-task"
    ));

    let abi_task = Task::new("sdk-abi-task", "raw.input", json!({}));
    let abi_handle = TaskHandle {
        task_id: abi_task.task_id.clone(),
        protocol_id: abi_task.protocol_id.clone(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: None,
        correlation_id: None,
    };
    let response = typed_response_bytes(&[
        (
            Opcode::TaskSubmitBatch,
            serde_json::to_value([abi_handle]).unwrap(),
        ),
        (Opcode::TaskCancel, serde_json::Value::Null),
        (Opcode::TaskOutcome, serde_json::Value::Null),
    ]);
    let abi = AbiTaskClient::new(Cursor::new(response), Cursor::new(Vec::<u8>::new()));

    let submitted = mutsuki_runtime_sdk::TaskSubmitter::submit_one(&abi, abi_task).unwrap();
    assert_eq!(submitted.task_id, "sdk-abi-task");
    mutsuki_runtime_sdk::TaskSubmitter::cancel_task(&abi, &submitted).unwrap();
    assert!(
        mutsuki_runtime_sdk::TaskSubmitter::task_outcome(&abi, &submitted)
            .unwrap()
            .is_none()
    );
}

#[test]
fn task_clients_submit_batch_across_local_and_abi_backends() {
    let runtime = Arc::new(Mutex::new(
        host_with_echo_runner()
            .into_runtime(runtime_profile())
            .unwrap(),
    ));
    let local = LocalTaskClient::new(runtime);
    let local_batch = TaskBatchBuilder::new("local-batch")
        .task(Task::new("local-batch-1", "raw.input", json!({})))
        .task(Task::new("local-batch-2", "raw.input", json!({})))
        .build();
    let local_handles = local.submit_batch(local_batch).unwrap();
    assert_eq!(local_handles.len(), 2);
    assert_eq!(local_handles[0].task_id, "local-batch-1");
    assert_eq!(local_handles[1].task_id, "local-batch-2");

    let abi_handles = vec![
        TaskHandle {
            task_id: "abi-batch-1".into(),
            protocol_id: "raw.input".into(),
            target_binding_id: None,
            cancel_policy: CancelPolicy::Cascade,
            trace_id: None,
            correlation_id: None,
        },
        TaskHandle {
            task_id: "abi-batch-2".into(),
            protocol_id: "raw.input".into(),
            target_binding_id: None,
            cancel_policy: CancelPolicy::Cascade,
            trace_id: None,
            correlation_id: None,
        },
    ];
    let response = typed_response_bytes(&[(
        Opcode::TaskSubmitBatch,
        serde_json::to_value(abi_handles).unwrap(),
    )]);
    let abi = AbiTaskClient::new(Cursor::new(response), Cursor::new(Vec::<u8>::new()));
    let abi_batch = TaskBatchBuilder::new("abi-batch")
        .task(Task::new("abi-batch-1", "raw.input", json!({})))
        .task(Task::new("abi-batch-2", "raw.input", json!({})))
        .build();
    let submitted = abi.submit_batch(abi_batch).unwrap();
    let (_reader, writer) = abi.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert_eq!(submitted.len(), 2);
    assert!(request.contains("\"method\":\"task.submit_batch\""));
    assert!(request.contains("\"batch_id\":\"abi-batch\""));
}

fn typed_response_bytes(responses: &[(Opcode, serde_json::Value)]) -> Vec<u8> {
    let hello = ProtocolHello::debug_jsonl();
    let ack: ProtocolHelloAck =
        serde_json::from_value(serde_json::to_value(hello).unwrap()).unwrap();
    let mut encoded =
        encode_jsonl_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS).unwrap();
    for (index, (opcode, value)) in responses.iter().enumerate() {
        encoded.extend(
            encode_jsonl_response(index as u64 + 2, *opcode, Ok(value), DEFAULT_WIRE_LIMITS)
                .unwrap(),
        );
    }
    encoded
}
