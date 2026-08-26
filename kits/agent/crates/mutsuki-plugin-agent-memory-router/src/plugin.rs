use mutsuki_agent_contracts::*;
use mutsuki_agent_sdk::{
    AgentMemoryActivateProtocol, AgentMemoryDeleteProtocol, AgentMemoryDisableProtocol,
    AgentMemoryFeedbackProtocol, AgentMemoryOpportunityProtocol, AgentMemoryQueryProtocol,
    AgentMemoryReviseProtocol, AgentMemoryWriteProtocol, orchestration_runner,
    service_result_event, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

use crate::MemoryRouter;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.memory_router";
pub const RUNNER_ID: &str = "mutsuki.agent.memory_router.runner";

pub fn plugin(client: RuntimeClientRef, router: MemoryRouter) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentMemoryQueryProtocol>()
        .protocol::<AgentMemoryWriteProtocol>()
        .protocol::<AgentMemoryActivateProtocol>()
        .protocol::<AgentMemoryDisableProtocol>()
        .protocol::<AgentMemoryDeleteProtocol>()
        .protocol::<AgentMemoryReviseProtocol>()
        .protocol::<AgentMemoryOpportunityProtocol>()
        .protocol::<AgentMemoryFeedbackProtocol>()
        .runner(Box::new(runner(client, router)))
}

pub fn runner(client: RuntimeClientRef, router: MemoryRouter) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentMemoryQueryProtocol>()
        .accepts::<AgentMemoryWriteProtocol>()
        .accepts::<AgentMemoryActivateProtocol>()
        .accepts::<AgentMemoryDisableProtocol>()
        .accepts::<AgentMemoryDeleteProtocol>()
        .accepts::<AgentMemoryReviseProtocol>()
        .accepts::<AgentMemoryOpportunityProtocol>()
        .accepts::<AgentMemoryFeedbackProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |_ctx, task| {
            let router = router.clone();
            Box::pin(async move { run_task(router, task).await })
        }),
    )
}

async fn run_task(router: MemoryRouter, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_MEMORY_QUERY_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.query_result",
            |request: AgentMemoryQueryRequest| router.query(request),
        ),
        AGENT_MEMORY_WRITE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.written",
            |request: AgentMemoryWriteRequest| router.write(request),
        ),
        AGENT_MEMORY_ACTIVATE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.activated",
            |request: AgentMemoryActivateRequest| router.activate(request),
        ),
        AGENT_MEMORY_DISABLE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.disabled",
            |request: AgentMemoryDisableRequest| router.disable(request),
        ),
        AGENT_MEMORY_DELETE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.deleted",
            |request: AgentMemoryDeleteRequest| router.delete(request),
        ),
        AGENT_MEMORY_REVISE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.revised",
            |request: AgentMemoryReviseRequest| router.revise(request),
        ),
        AGENT_MEMORY_OPPORTUNITY_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.opportunity",
            |request: AgentMemoryOpportunityRequest| {
                router.detect_opportunity(
                    request.session_id,
                    request.turn_id,
                    request.query,
                    request.mode,
                )
            },
        ),
        AGENT_MEMORY_FEEDBACK_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.memory.feedback",
            |request: MemoryAdoptionFeedback| {
                router.record_feedback(request.clone())?;
                Ok(request)
            },
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
