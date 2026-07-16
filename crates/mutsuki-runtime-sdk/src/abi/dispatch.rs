use mutsuki_runtime_wire::{AnyWireRequest, Opcode, decode_jsonl_any_request};

use crate::{ResourcePlanGateway, TaskSubmitter};

use super::error::{abi_failure, encode_result};

pub fn dispatch_host_request(
    task_submitter: &dyn TaskSubmitter,
    resource_gateway: &dyn ResourcePlanGateway,
    request: &[u8],
) -> Vec<u8> {
    let decoded = match decode_jsonl_any_request(request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS)
    {
        Ok(decoded) => decoded,
        Err(error) => return uncorrelated_error(error.to_string()),
    };
    let request_id = decoded.request_id;
    match decoded.request {
        AnyWireRequest::SubmitTaskBatch(request) => encode_result(
            request_id,
            Opcode::TaskSubmitBatch,
            task_submitter.submit_batch(request.batch),
        ),
        AnyWireRequest::CancelTask(request) => encode_result(
            request_id,
            Opcode::TaskCancel,
            task_submitter.cancel_task(&request.handle),
        ),
        AnyWireRequest::TaskOutcome(request) => encode_result(
            request_id,
            Opcode::TaskOutcome,
            task_submitter.task_outcome(&request.handle),
        ),
        AnyWireRequest::CollectReadPlan(request) => encode_result(
            request_id,
            Opcode::ResourceReadCollect,
            resource_gateway.collect_read_plan(&request.plan),
        ),
        AnyWireRequest::SnapshotReadPlan(request) => encode_result(
            request_id,
            Opcode::ResourceReadSnapshot,
            resource_gateway.snapshot_read_plan(&request.plan, &request.kind_id, &request.schema),
        ),
        AnyWireRequest::OpenStreamPlan(request) => encode_result(
            request_id,
            Opcode::ResourceStreamOpen,
            resource_gateway.open_stream_plan(&request.plan),
        ),
        AnyWireRequest::ExportPlan(request) => encode_result(
            request_id,
            Opcode::ResourceExport,
            resource_gateway.execute_export_plan(&request.plan),
        ),
        AnyWireRequest::CommitWritePlan(request) => encode_result(
            request_id,
            Opcode::ResourceWriteCommit,
            resource_gateway.commit_write_plan(&request.plan, request.bytes),
        ),
        AnyWireRequest::CommandPlan(request) => encode_result(
            request_id,
            Opcode::ResourceCommand,
            resource_gateway.execute_command_plan(&request.plan),
        ),
        AnyWireRequest::CommandBatch(request) => encode_result(
            request_id,
            Opcode::ResourceCommandBatch,
            resource_gateway.execute_command_batch(&request.batch),
        ),
        AnyWireRequest::SagaPlan(request) => encode_result(
            request_id,
            Opcode::ResourceSaga,
            resource_gateway.execute_saga_plan(&request.saga),
        ),
        unsupported => encode_result::<()>(
            request_id,
            unsupported.opcode(),
            Err(abi_failure(
                "abi.host_opcode_unsupported",
                format!(
                    "unsupported host opcode {:#06x}",
                    unsupported.opcode() as u16
                ),
            )),
        ),
    }
}

fn uncorrelated_error(detail: String) -> Vec<u8> {
    let failure = abi_failure("abi.host_decode", detail);
    // A malformed frame has no trustworthy request id. ABI callback status remains successful so
    // the bounded structured diagnostic reaches the peer; typed peers reject id zero immediately.
    serde_json::to_vec(failure.error()).unwrap_or_default()
}
