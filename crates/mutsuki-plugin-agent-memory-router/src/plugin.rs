use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentMemoryActivateProtocol, AgentMemoryQueryProtocol, AgentMemoryWriteProtocol,
    orchestration_runner, service_result_event, unsupported_protocol,
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
        .runner(Box::new(runner(client, router)))
}

pub fn runner(client: RuntimeClientRef, router: MemoryRouter) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentMemoryQueryProtocol>()
        .accepts::<AgentMemoryWriteProtocol>()
        .accepts::<AgentMemoryActivateProtocol>()
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
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
