use serde_json::{Value, json};

use crate::{BINARY_CODEC_ID, DEBUG_JSONL_CODEC_ID, Opcode, SCHEMA_REVISION, WireProtocolVersion};

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
