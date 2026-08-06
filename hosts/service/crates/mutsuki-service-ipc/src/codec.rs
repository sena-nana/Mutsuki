use mutsuki_service_control::{
    ControlCommand, ControlErrorBody, ControlMethod, ControlRequest, ControlResponse,
    ControlResult, CoreDrainResponse, CoreStatus, EventSourceStatus, HealthReport, HostMetrics,
    IdParam, LogTailParams, LogTailResponse, PluginDeploymentClearParam, PluginDeploymentParam,
    PluginListResponse, PluginReloadResponse, RunnerStatus, RuntimeStatisticsView, ServiceStatus,
    TaskEventPage, TaskEventsAfterParam, TaskOutcomeView, TaskOutcomesBatchParam,
    TaskOutcomesBatchResponse, TaskSnapshot, TaskSubmitBatchParam, TaskSubmitBatchResponse,
    TaskWaitParam, TaskWaitResponse,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::error::{IpcError, IpcResult};
use crate::frame::{FrameFlags, OPCODE_CANCEL, encode_frame, encode_frame_into};
use crate::limits::ControlIpcLimits;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct EmptyPayload {}

#[derive(Deserialize)]
struct AuthenticatedRequest<T> {
    token: String,
    request: T,
}

#[derive(Serialize)]
struct AuthenticatedRequestRef<'a, T> {
    token: &'a str,
    request: &'a T,
}

pub fn encode_binary_request_with_scratch(
    request_id: u64,
    request: &ControlRequest,
    limits: ControlIpcLimits,
    frame_buf: &mut Vec<u8>,
    payload_buf: &mut Vec<u8>,
) -> IpcResult<()> {
    payload_buf.clear();
    macro_rules! encode_request {
        ($payload:expr) => {
            encode_messagepack_into(
                &AuthenticatedRequestRef {
                    token: &request.token,
                    request: $payload,
                },
                payload_buf,
                limits,
            )?
        };
    }
    match &request.command {
        ControlCommand::ServiceStatus
        | ControlCommand::ServiceShutdown
        | ControlCommand::CoreStatus
        | ControlCommand::PluginList
        | ControlCommand::PluginReload
        | ControlCommand::RunnerList
        | ControlCommand::EventSourceList
        | ControlCommand::CoreBeginDrain
        | ControlCommand::TaskList
        | ControlCommand::HealthCheck
        | ControlCommand::RuntimeStatistics
        | ControlCommand::HostMetrics => encode_request!(&EmptyPayload {}),
        ControlCommand::PluginDeploymentSet(value) => encode_request!(value),
        ControlCommand::PluginDeploymentClear(value) => encode_request!(value),
        ControlCommand::RunnerRestart(value)
        | ControlCommand::RunnerStop(value)
        | ControlCommand::EventSourceRestart(value)
        | ControlCommand::TaskCancel(value)
        | ControlCommand::TaskOutcome(value) => encode_request!(value),
        ControlCommand::TaskSubmitBatch(value) => encode_request!(value),
        ControlCommand::TaskEventsAfter(value) => encode_request!(value),
        ControlCommand::LogTail(value) => encode_request!(value),
        ControlCommand::TaskOutcomesBatch(value) => encode_request!(value),
        ControlCommand::TaskWait(value) => encode_request!(value),
    }
    encode_frame_into(
        frame_buf,
        request.method().opcode(),
        FrameFlags::REQUEST,
        request_id,
        payload_buf,
        limits,
    )
}

pub fn encode_binary_response_with_scratch(
    request_id: u64,
    method: ControlMethod,
    response: &ControlResponse,
    limits: ControlIpcLimits,
    frame_buf: &mut Vec<u8>,
    payload_buf: &mut Vec<u8>,
) -> IpcResult<()> {
    payload_buf.clear();
    let flags = match response {
        ControlResponse::Error(error) => {
            encode_messagepack_into(error, payload_buf, limits)?;
            FrameFlags::RESPONSE | FrameFlags::ERROR
        }
        ControlResponse::Ok(result) => {
            if result.method() != method {
                return Err(IpcError::Protocol(format!(
                    "control response {:?} does not match {:?}",
                    result.method(),
                    method
                )));
            }
            match result {
                ControlResult::ServiceStatus(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::ServiceShutdown
                | ControlResult::RunnerRestart
                | ControlResult::RunnerStop
                | ControlResult::EventSourceRestart
                | ControlResult::TaskCancel => {
                    encode_messagepack_into(&EmptyPayload {}, payload_buf, limits)?
                }
                ControlResult::CoreStatus(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::PluginList(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::PluginReload(value)
                | ControlResult::PluginDeploymentSet(value)
                | ControlResult::PluginDeploymentClear(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::RunnerList(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::EventSourceList(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::CoreBeginDrain(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::TaskSubmitBatch(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::TaskList(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::TaskOutcome(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::TaskEventsAfter(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::HealthCheck(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::LogTail(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::TaskOutcomesBatch(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::TaskWait(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::RuntimeStatistics(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
                ControlResult::HostMetrics(value) => {
                    encode_messagepack_into(value, payload_buf, limits)?
                }
            }
            FrameFlags::RESPONSE
        }
    };
    encode_frame_into(
        frame_buf,
        method.opcode(),
        flags,
        request_id,
        payload_buf,
        limits,
    )
}

pub fn decode_binary_request(
    method: ControlMethod,
    payload: &[u8],
    limits: ControlIpcLimits,
) -> IpcResult<ControlRequest> {
    macro_rules! decode_request {
        ($ty:ty, $variant:path) => {{
            let body: AuthenticatedRequest<$ty> = decode_messagepack(payload, limits)?;
            ControlRequest::new(body.token, $variant(body.request))
        }};
    }
    macro_rules! decode_empty {
        ($variant:path) => {{
            let body: AuthenticatedRequest<EmptyPayload> = decode_messagepack(payload, limits)?;
            ControlRequest::new(body.token, $variant)
        }};
    }
    Ok(match method {
        ControlMethod::ServiceStatus => decode_empty!(ControlCommand::ServiceStatus),
        ControlMethod::ServiceShutdown => decode_empty!(ControlCommand::ServiceShutdown),
        ControlMethod::CoreStatus => decode_empty!(ControlCommand::CoreStatus),
        ControlMethod::PluginList => decode_empty!(ControlCommand::PluginList),
        ControlMethod::PluginReload => decode_empty!(ControlCommand::PluginReload),
        ControlMethod::PluginDeploymentSet => {
            decode_request!(PluginDeploymentParam, ControlCommand::PluginDeploymentSet)
        }
        ControlMethod::PluginDeploymentClear => decode_request!(
            PluginDeploymentClearParam,
            ControlCommand::PluginDeploymentClear
        ),
        ControlMethod::RunnerList => decode_empty!(ControlCommand::RunnerList),
        ControlMethod::RunnerRestart => decode_request!(IdParam, ControlCommand::RunnerRestart),
        ControlMethod::RunnerStop => decode_request!(IdParam, ControlCommand::RunnerStop),
        ControlMethod::EventSourceList => decode_empty!(ControlCommand::EventSourceList),
        ControlMethod::EventSourceRestart => {
            decode_request!(IdParam, ControlCommand::EventSourceRestart)
        }
        ControlMethod::CoreBeginDrain => decode_empty!(ControlCommand::CoreBeginDrain),
        ControlMethod::TaskSubmitBatch => {
            decode_request!(TaskSubmitBatchParam, ControlCommand::TaskSubmitBatch)
        }
        ControlMethod::TaskList => decode_empty!(ControlCommand::TaskList),
        ControlMethod::TaskCancel => decode_request!(IdParam, ControlCommand::TaskCancel),
        ControlMethod::TaskOutcome => decode_request!(IdParam, ControlCommand::TaskOutcome),
        ControlMethod::TaskEventsAfter => {
            decode_request!(TaskEventsAfterParam, ControlCommand::TaskEventsAfter)
        }
        ControlMethod::HealthCheck => decode_empty!(ControlCommand::HealthCheck),
        ControlMethod::LogTail => decode_request!(LogTailParams, ControlCommand::LogTail),
        ControlMethod::TaskOutcomesBatch => {
            decode_request!(TaskOutcomesBatchParam, ControlCommand::TaskOutcomesBatch)
        }
        ControlMethod::TaskWait => decode_request!(TaskWaitParam, ControlCommand::TaskWait),
        ControlMethod::RuntimeStatistics => decode_empty!(ControlCommand::RuntimeStatistics),
        ControlMethod::HostMetrics => decode_empty!(ControlCommand::HostMetrics),
    })
}

pub fn decode_binary_response(
    method: ControlMethod,
    payload: &[u8],
    is_error: bool,
    limits: ControlIpcLimits,
) -> IpcResult<ControlResponse> {
    if is_error {
        return Ok(ControlResponse::Error(decode_messagepack::<
            ControlErrorBody,
        >(payload, limits)?));
    }
    macro_rules! decode_result {
        ($ty:ty, $variant:path) => {
            ControlResponse::Ok($variant(decode_messagepack::<$ty>(payload, limits)?))
        };
    }
    macro_rules! decode_empty_result {
        ($variant:path) => {{
            let _: EmptyPayload = decode_messagepack(payload, limits)?;
            ControlResponse::Ok($variant)
        }};
    }
    Ok(match method {
        ControlMethod::ServiceStatus => decode_result!(ServiceStatus, ControlResult::ServiceStatus),
        ControlMethod::ServiceShutdown => decode_empty_result!(ControlResult::ServiceShutdown),
        ControlMethod::CoreStatus => decode_result!(CoreStatus, ControlResult::CoreStatus),
        ControlMethod::PluginList => decode_result!(PluginListResponse, ControlResult::PluginList),
        ControlMethod::PluginReload => {
            decode_result!(PluginReloadResponse, ControlResult::PluginReload)
        }
        ControlMethod::PluginDeploymentSet => {
            decode_result!(PluginReloadResponse, ControlResult::PluginDeploymentSet)
        }
        ControlMethod::PluginDeploymentClear => {
            decode_result!(PluginReloadResponse, ControlResult::PluginDeploymentClear)
        }
        ControlMethod::RunnerList => decode_result!(Vec<RunnerStatus>, ControlResult::RunnerList),
        ControlMethod::RunnerRestart => decode_empty_result!(ControlResult::RunnerRestart),
        ControlMethod::RunnerStop => decode_empty_result!(ControlResult::RunnerStop),
        ControlMethod::EventSourceList => {
            decode_result!(Vec<EventSourceStatus>, ControlResult::EventSourceList)
        }
        ControlMethod::EventSourceRestart => {
            decode_empty_result!(ControlResult::EventSourceRestart)
        }
        ControlMethod::CoreBeginDrain => {
            decode_result!(CoreDrainResponse, ControlResult::CoreBeginDrain)
        }
        ControlMethod::TaskSubmitBatch => {
            decode_result!(TaskSubmitBatchResponse, ControlResult::TaskSubmitBatch)
        }
        ControlMethod::TaskList => decode_result!(Vec<TaskSnapshot>, ControlResult::TaskList),
        ControlMethod::TaskCancel => decode_empty_result!(ControlResult::TaskCancel),
        ControlMethod::TaskOutcome => decode_result!(TaskOutcomeView, ControlResult::TaskOutcome),
        ControlMethod::TaskEventsAfter => {
            decode_result!(TaskEventPage, ControlResult::TaskEventsAfter)
        }
        ControlMethod::HealthCheck => decode_result!(HealthReport, ControlResult::HealthCheck),
        ControlMethod::LogTail => decode_result!(LogTailResponse, ControlResult::LogTail),
        ControlMethod::TaskOutcomesBatch => {
            decode_result!(TaskOutcomesBatchResponse, ControlResult::TaskOutcomesBatch)
        }
        ControlMethod::TaskWait => decode_result!(TaskWaitResponse, ControlResult::TaskWait),
        ControlMethod::RuntimeStatistics => {
            decode_result!(RuntimeStatisticsView, ControlResult::RuntimeStatistics)
        }
        ControlMethod::HostMetrics => decode_result!(HostMetrics, ControlResult::HostMetrics),
    })
}

pub fn encode_binary_cancel(request_id: u64, limits: ControlIpcLimits) -> IpcResult<Vec<u8>> {
    encode_frame(
        OPCODE_CANCEL,
        FrameFlags::CANCEL,
        request_id,
        Vec::new(),
        limits,
    )
}

fn encode_messagepack_into<T: Serialize>(
    value: &T,
    buf: &mut Vec<u8>,
    limits: ControlIpcLimits,
) -> IpcResult<()> {
    let mut serializer = rmp_serde::Serializer::new(&mut *buf).with_struct_map();
    serializer.unstable_set_max_depth(limits.max_msgpack_nesting_depth);
    value.serialize(&mut serializer)?;
    if buf.len() > limits.max_payload_bytes {
        return Err(IpcError::PayloadOversized {
            actual: buf.len(),
            limit: limits.max_payload_bytes,
        });
    }
    Ok(())
}

fn decode_messagepack<T: DeserializeOwned>(
    payload: &[u8],
    limits: ControlIpcLimits,
) -> IpcResult<T> {
    let mut deserializer = rmp_serde::Deserializer::new(payload);
    deserializer.set_max_depth(limits.max_msgpack_nesting_depth);
    Ok(T::deserialize(&mut deserializer)?)
}
