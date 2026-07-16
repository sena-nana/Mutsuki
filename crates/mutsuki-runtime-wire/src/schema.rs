use serde_json::{Value, json};

mod golden;
pub use golden::{generated_binary_golden_json, generated_binary_golden_value};

use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CancelPolicy, CompletionBatch, DispatchLane, EntryCompletion,
    OrderingRequirement, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
    ResourceSealState, ResourceSemantic, RunnerContext, RunnerResult, RuntimeError, Task,
    TaskBatch, TaskHandle, TaskLease, WorkBatch, WorkResourcePlan,
};

use crate::{
    BINARY_CODEC_ID, CancelRunnerRequest, DEBUG_JSONL_CODEC_ID, DisposeRunnerRequest,
    InitializeRequest, Opcode, ProtocolHello, ProtocolHelloAck, RunBatchRequest, SCHEMA_REVISION,
    SubmitTaskBatchRequest, WireProtocolVersion,
};

pub fn generated_schema_value() -> Value {
    let operations = Opcode::ALL
        .into_iter()
        .map(|opcode| {
            let (request, response) = operation_types(opcode);
            json!({
                "opcode": opcode as u16,
                "method": opcode.method(),
                "management": opcode.is_management(),
                "request": request,
                "response": response,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_revision": SCHEMA_REVISION,
        "protocol": {
            "major": WireProtocolVersion::CURRENT.major,
            "minor": WireProtocolVersion::CURRENT.minor,
            "compatibility": {
                "unknown_additive_fields": "ignore",
                "removed_or_changed_required_fields": "major_breaking",
                "opcode_policy": "append_only_never_reuse"
            }
        },
        "codecs": [DEBUG_JSONL_CODEC_ID, BINARY_CODEC_ID],
        "operations": operations,
        "resource_policy": {
            "large_bytes": "ResourceRef_or_stream_descriptor",
            "inline_limit_bytes": crate::MAX_INLINE_RESOURCE_BYTES
        },
        "limits": {
            "max_frame_bytes": crate::MAX_FRAME_BYTES,
            "max_payload_bytes": crate::MAX_PAYLOAD_BYTES,
            "max_jsonl_line_bytes": crate::MAX_JSONL_LINE_BYTES,
            "max_in_flight_requests": crate::MAX_IN_FLIGHT_REQUESTS,
            "management_reserved_requests": crate::MANAGEMENT_RESERVED_REQUESTS
        }
    })
}

pub fn generated_schema_json() -> String {
    let mut output = serde_json::to_string_pretty(&generated_schema_value())
        .expect("runtime wire schema must serialize");
    output.push('\n');
    output
}

pub fn generated_fixtures_value() -> Value {
    let mut task = Task::new(
        "task-wire-1",
        "mutsuki.fixture.echo",
        json!({"message": "hello"}),
    );
    task.lease_id = Some("lease-wire-1".into());
    task.trace_id = Some("trace-wire-1".into());
    task.correlation_id = Some("corr-wire-1".into());
    task.registry_generation = 7;
    let lease = TaskLease {
        lease_id: "lease-wire-1".into(),
        task_id: task.task_id.clone(),
        runner_id: "fixture.runner".into(),
        executor_id: "executor:fixture".into(),
        registry_generation: 7,
        acquired_at_step: 11,
        expires_at_step: Some(21),
    };
    let batch = WorkBatch {
        batch_id: "batch-wire-1".into(),
        tick_id: "tick-11".into(),
        batch_key: "fixture.runner".into(),
        entries: vec![BatchEntry {
            entry_id: "entry-wire-1".into(),
            task_id: task.task_id.clone(),
            trace_id: task.trace_id.clone(),
            parent_id: None,
            payload_index: 0,
            resource_requirement_indices: Vec::new(),
            cancel_index: Some(0),
            deadline_tick: Some(20),
            priority: task.priority,
            lane: DispatchLane::Normal,
            ordering: OrderingRequirement::None,
        }],
        payload: BatchPayload::from_tasks(std::slice::from_ref(&task)),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: vec![lease],
    };
    let mut ctx = RunnerContext::new(
        7,
        11,
        "executor:fixture",
        Some("lease-wire-1".into()),
        "invocation-wire-1",
    )
    .with_batch(batch.batch_id.clone(), 1);
    ctx.deadline_tick = Some(20);
    let completion = CompletionBatch {
        batch_id: batch.batch_id.clone(),
        tick_id: batch.tick_id.clone(),
        results: vec![EntryCompletion {
            entry_id: "entry-wire-1".into(),
            task_id: task.task_id.clone(),
            result: Some(RunnerResult::completed(task.task_id.clone())),
            error: None,
        }],
        metadata: Vec::new(),
    };
    let handle = TaskHandle {
        task_id: task.task_id.clone(),
        protocol_id: task.protocol_id.clone(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: task.trace_id.clone(),
        correlation_id: task.correlation_id.clone(),
    };
    let resource = ResourceRef {
        ref_id: "resource-wire-1".into(),
        resource_id: ResourceId {
            kind_id: "fixture.bytes".into(),
            slot_id: "slot-wire-1".into(),
            generation: 7,
            version: 3,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: "fixture.provider".into(),
        resource_kind: "fixture.bytes".into(),
        schema: "fixture.bytes.v1".into(),
        version: 3,
        generation: 7,
        access: ResourceAccess::Blob {
            store_id: "fixture.store".into(),
            key: "blob-wire-1".into(),
        },
        size_hint: Some(1_048_576),
        content_hash: Some("sha256:fixture".into()),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    };
    let runtime_error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "fixture",
        "fixture.route",
    );
    let hello = ProtocolHello::debug_jsonl();
    let fixtures = vec![
        fixture("ProtocolHello", &hello),
        fixture(
            "InitializeRequest",
            &InitializeRequest {
                hello: hello.clone(),
                config: Some(json!({"mode": "fixture"})),
            },
        ),
        fixture("ProtocolHelloAck", &ProtocolHelloAck::accept(&hello, None)),
        fixture(
            "RunBatchRequest",
            &RunBatchRequest {
                runner_id: "fixture.runner".into(),
                ctx,
                batch,
            },
        ),
        fixture("CompletionBatch", &completion),
        fixture(
            "CancelRunnerRequest",
            &CancelRunnerRequest {
                runner_id: "fixture.runner".into(),
                invocation_id: "invocation-wire-1".into(),
            },
        ),
        fixture(
            "DisposeRunnerRequest",
            &DisposeRunnerRequest {
                runner_id: "fixture.runner".into(),
            },
        ),
        fixture(
            "SubmitTaskBatchRequest",
            &SubmitTaskBatchRequest {
                batch: TaskBatch::one("submit-wire-1", task),
            },
        ),
        fixture("TaskHandle", &handle),
        fixture("ResourceRef", &resource),
        fixture("RuntimeError", &runtime_error),
    ];
    json!({
        "schema_revision": SCHEMA_REVISION,
        "fixtures": fixtures,
    })
}

pub fn generated_fixtures_json() -> String {
    let mut output = serde_json::to_string_pretty(&generated_fixtures_value())
        .expect("runtime wire fixtures must serialize");
    output.push('\n');
    output
}

fn fixture(name: &str, value: &impl serde::Serialize) -> Value {
    json!({
        "type": name,
        "value": serde_json::to_value(value).expect("fixture must serialize"),
    })
}

fn operation_types(opcode: Opcode) -> (&'static str, &'static str) {
    match opcode {
        Opcode::PluginInitialize => ("InitializeRequest", "ProtocolHelloAck"),
        Opcode::RunnerRunBatch => ("RunBatchRequest", "CompletionBatch"),
        Opcode::RunnerCancel => ("CancelRunnerRequest", "Unit"),
        Opcode::RunnerDispose => ("DisposeRunnerRequest", "Unit"),
        Opcode::TaskSubmitBatch => ("SubmitTaskBatchRequest", "TaskHandle[]"),
        Opcode::TaskCancel => ("CancelTaskRequest", "Unit"),
        Opcode::TaskOutcome => ("TaskOutcomeRequest", "TaskOutcome?"),
        Opcode::ResourceReadCollect => ("CollectReadPlanRequest", "Bytes"),
        Opcode::ResourceReadSnapshot => ("SnapshotReadPlanRequest", "SnapshotDescriptor"),
        Opcode::ResourceStreamOpen => ("OpenStreamPlanRequest", "StreamPlan"),
        Opcode::ResourceExport => ("ExportPlanRequest", "PlanReceipt"),
        Opcode::ResourceWriteCommit => ("CommitWritePlanRequest", "PlanReceipt"),
        Opcode::ResourceCommand => ("CommandPlanRequest", "PlanReceipt"),
        Opcode::ResourceCommandBatch => ("CommandBatchRequest", "PlanReceipt[]"),
        Opcode::ResourceSaga => ("SagaPlanRequest", "PlanReceipt[]"),
        Opcode::ResourceCreateBlob => ("CreateBlobRequest", "ResourceRef"),
        Opcode::ResourceCreateCowState => ("CreateCowStateRequest", "ResourceRef"),
        Opcode::ResourceCreateCapability => ("CreateCapabilityRequest", "ResourceRef"),
    }
}
