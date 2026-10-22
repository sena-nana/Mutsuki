use std::io::Cursor;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_wire::{
    BINARY_CODEC_ID, DEFAULT_WIRE_LIMITS, Opcode, ProtocolHello, ProtocolHelloAck,
    SubmitTaskBatchRequest, decode_binary_frame, decode_binary_request, encode_binary_response,
};
use serde_json::json;

use crate::AbiTaskClient;
use mutsuki_runtime_sdk::TaskBatchBuilder;
use mutsuki_runtime_sdk::TaskSubmitter;

use super::helpers::{host_with_echo_runner, runtime_profile};

#[test]
fn host_task_clients_share_task_contract_across_local_and_abi_backends() {
    // The in-process client is the Host actor's submitter: there is no
    // direct `CoreRuntime` client that could bypass the actor.
    let host = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    let local = host.host_context().task_submitter_ref();
    let mut local_task = Task::new("local-client-task", "raw.input", json!({"source": "local"}));
    local_task.trace_id = Some("trace-local".into());
    local_task.correlation_id = Some("corr-local".into());

    let local_handle = local.submit_task(local_task).unwrap();

    assert_eq!(local_handle.task_id, "local-client-task");
    assert_eq!(local_handle.protocol_id, "raw.input");
    assert_eq!(
        local_handle.trace_id.as_ref().map(|id| id.as_str()),
        Some("trace-local")
    );
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
    let bytes = writer.into_inner();
    let frames = split_frames(&bytes);
    let (_, submit_request) =
        decode_binary_request::<SubmitTaskBatchRequest>(frames[1], DEFAULT_WIRE_LIMITS).unwrap();

    assert_eq!(submitted.task_id, "abi-client-task");
    assert_eq!(
        submitted.trace_id.as_ref().map(|id| id.as_str()),
        Some("trace-abi")
    );
    assert!(matches!(
        outcome,
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "abi-client-task"
    ));
    assert_eq!(
        decode_binary_frame(frames[1], DEFAULT_WIRE_LIMITS)
            .unwrap()
            .header
            .opcode,
        Opcode::TaskSubmitBatch
    );
    assert_eq!(
        decode_binary_frame(frames[2], DEFAULT_WIRE_LIMITS)
            .unwrap()
            .header
            .opcode,
        Opcode::TaskCancel
    );
    assert_eq!(
        decode_binary_frame(frames[3], DEFAULT_WIRE_LIMITS)
            .unwrap()
            .header
            .opcode,
        Opcode::TaskOutcome
    );
    assert_eq!(
        submit_request.batch.tasks[0]
            .trace_id
            .as_ref()
            .map(|id| id.as_str()),
        Some("trace-abi")
    );
}

#[test]
fn task_clients_implement_sdk_task_submitter_boundary() {
    // The in-process client is the Host actor's submitter: there is no
    // direct `CoreRuntime` client that could bypass the actor.
    let host = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    let local = host.host_context().task_submitter_ref();
    let local_handle = mutsuki_runtime_sdk::TaskSubmitter::submit_task(
        local.as_ref(),
        Task::new("sdk-local-task", "raw.input", json!({})),
    )
    .unwrap();
    mutsuki_runtime_sdk::TaskSubmitter::cancel_task(local.as_ref(), &local_handle).unwrap();

    assert_eq!(local_handle.task_id, "sdk-local-task");
    assert!(matches!(
        mutsuki_runtime_sdk::TaskSubmitter::task_outcome(local.as_ref(), &local_handle).unwrap(),
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
    // The in-process client is the Host actor's submitter: there is no
    // direct `CoreRuntime` client that could bypass the actor.
    let host = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    let local = host.host_context().task_submitter_ref();
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
    let bytes = writer.into_inner();
    let frames = split_frames(&bytes);
    let (_, request) =
        decode_binary_request::<SubmitTaskBatchRequest>(frames[1], DEFAULT_WIRE_LIMITS).unwrap();

    assert_eq!(submitted.len(), 2);
    assert_eq!(request.batch.batch_id, "abi-batch");
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

#[test]
fn deferred_host_client_fails_until_bound_then_forwards_through_the_actor() {
    let deferred = crate::DeferredHostClient::new("test.embedder");
    let task = Task::new("deferred-task", "raw.input", json!({}));

    let unbound = TaskSubmitter::submit_task(&deferred, task.clone())
        .expect_err("an unbound client has no actor to submit to");
    assert_eq!(unbound.error().route, "runtime_client.not_bound");
    assert_eq!(unbound.error().source, "test.embedder");

    let host = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();
    deferred.bind(host.host_context()).unwrap();
    let rebound = deferred
        .bind(host.host_context())
        .expect_err("binding twice would silently swap the actor behind live callers");
    assert_eq!(rebound.error().route, "runtime_client.already_bound");

    let handle = TaskSubmitter::submit_task(&deferred, task).unwrap();
    TaskSubmitter::cancel_task(&deferred, &handle).unwrap();
    assert!(matches!(
        TaskSubmitter::task_outcome(&deferred, &handle).unwrap(),
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "deferred-task"
    ));
}

#[test]
fn task_handle_future_is_woken_by_the_host_actor_outside_a_runner() {
    // A top-level await (console RPC, event source) runs on a current-thread executor with no
    // runner continuation: only the actor's waker registration can resume it.
    let runner_descriptor = super::helpers::descriptor("slow.runner", "slow.input");
    let mut bootstrapper = crate::RuntimeBootstrapper::new();
    bootstrapper.register_manifest(crate::runner_manifest(
        "plugin-a",
        vec![runner_descriptor.clone()],
    ));
    bootstrapper.register_runner(Box::new(crate::NativeRunner::new(
        runner_descriptor,
        |_ctx: mutsuki_runtime_core::RunnerContext, task| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok(RunnerResult::completed(task.task_id))
        },
    )));
    let host = bootstrapper
        .into_host_runtime_with_config(
            runtime_profile(),
            crate::HostRuntimeConfig {
                event_driven: true,
                ..crate::HostRuntimeConfig::default()
            },
        )
        .unwrap();
    let client = std::sync::Arc::new(crate::DeferredHostClient::new("test.host"));
    client.bind(host.host_context()).unwrap();
    let handle = client
        .submit_task(Task::new("slow-task", "slow.input", json!({})))
        .unwrap();
    let future = mutsuki_runtime_sdk::TaskHandleFuture::new(client, handle);

    let started = std::time::Instant::now();
    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async { tokio::time::timeout(std::time::Duration::from_secs(5), future).await })
        .expect("actor wakes the parked future")
        .unwrap();

    assert!(matches!(outcome, TaskOutcome::Completed { task_id, .. } if task_id == "slow-task"));
    // `Timeout` re-polls its inner future when the deadline fires, so only the elapsed time proves
    // the completion woke the future rather than the timer.
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}
