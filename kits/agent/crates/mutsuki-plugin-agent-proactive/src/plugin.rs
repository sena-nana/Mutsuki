use mutsuki_agent_contracts::*;
use mutsuki_agent_sdk::{
    AgentScheduleCancelProtocol, AgentScheduleCreateProtocol, AgentScheduleDueProtocol,
    AgentScheduleGetProtocol, AgentScheduleHistoryProtocol, AgentScheduleListProtocol,
    AgentSchedulePauseProtocol, AgentScheduleResumeProtocol, AgentScheduleUpdateProtocol,
    orchestration_runner, service_result_event, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};
use serde::Deserialize;

use crate::ProactiveScheduleService;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.proactive";
pub const RUNNER_ID: &str = "mutsuki.agent.proactive.runner";

pub fn plugin(client: RuntimeClientRef, service: ProactiveScheduleService) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentScheduleCreateProtocol>()
        .protocol::<AgentScheduleListProtocol>()
        .protocol::<AgentScheduleGetProtocol>()
        .protocol::<AgentScheduleUpdateProtocol>()
        .protocol::<AgentSchedulePauseProtocol>()
        .protocol::<AgentScheduleResumeProtocol>()
        .protocol::<AgentScheduleCancelProtocol>()
        .protocol::<AgentScheduleHistoryProtocol>()
        .protocol::<AgentScheduleDueProtocol>()
        .runner(Box::new(runner(client, service)))
}

pub fn runner(
    client: RuntimeClientRef,
    service: ProactiveScheduleService,
) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentScheduleCreateProtocol>()
        .accepts::<AgentScheduleListProtocol>()
        .accepts::<AgentScheduleGetProtocol>()
        .accepts::<AgentScheduleUpdateProtocol>()
        .accepts::<AgentSchedulePauseProtocol>()
        .accepts::<AgentScheduleResumeProtocol>()
        .accepts::<AgentScheduleCancelProtocol>()
        .accepts::<AgentScheduleHistoryProtocol>()
        .accepts::<AgentScheduleDueProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |_ctx, task| {
            let service = service.clone();
            Box::pin(async move { run_task(service, task).await })
        }),
    )
}

#[derive(Clone, Debug, Deserialize)]
struct TimedScheduleRequest<T> {
    request: T,
    now_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct DueRequest {
    schedule_id: String,
    due_at_unix_ms: u64,
    epoch: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct CompleteRequest {
    result: ScheduledRunResult,
    now_unix_ms: u64,
}

async fn run_task(service: ProactiveScheduleService, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_SCHEDULE_CREATE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.created",
            |payload: TimedScheduleRequest<CreateScheduleRequest>| {
                service.create(payload.request, payload.now_unix_ms)
            },
        ),
        AGENT_SCHEDULE_LIST_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.listed",
            |request: ListSchedulesRequest| service.list(request),
        ),
        AGENT_SCHEDULE_GET_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.loaded",
            |request: ScheduleIdRequest| service.get(request),
        ),
        AGENT_SCHEDULE_UPDATE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.updated",
            |payload: TimedScheduleRequest<UpdateScheduleRequest>| {
                service.update(payload.request, payload.now_unix_ms)
            },
        ),
        AGENT_SCHEDULE_PAUSE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.paused",
            |payload: TimedScheduleRequest<ScheduleIdRequest>| {
                service.pause(payload.request, payload.now_unix_ms)
            },
        ),
        AGENT_SCHEDULE_RESUME_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.resumed",
            |payload: TimedScheduleRequest<ScheduleIdRequest>| {
                service.resume(payload.request, payload.now_unix_ms)
            },
        ),
        AGENT_SCHEDULE_CANCEL_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.cancelled",
            |payload: TimedScheduleRequest<ScheduleIdRequest>| {
                service.cancel(payload.request, payload.now_unix_ms)
            },
        ),
        AGENT_SCHEDULE_HISTORY_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.history",
            |request: ScheduleIdRequest| service.history(request),
        ),
        AGENT_SCHEDULE_DUE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.due",
            |request: DueRequest| {
                service.due(&request.schedule_id, request.due_at_unix_ms, request.epoch)
            },
        ),
        "mutsuki.agent.schedule/complete@1" => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.schedule.completed",
            |request: CompleteRequest| {
                service.complete_execution(request.result, request.now_unix_ms)
            },
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
