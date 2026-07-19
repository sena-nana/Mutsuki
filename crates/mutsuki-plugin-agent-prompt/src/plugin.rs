use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentPromptGetProtocol, AgentPromptRenderProtocol, orchestration_runner, service_result_event,
    unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

use crate::PromptRegistry;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.prompt";
pub const RUNNER_ID: &str = "mutsuki.agent.prompt.runner";

pub fn plugin(client: RuntimeClientRef, registry: PromptRegistry) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentPromptRenderProtocol>()
        .protocol::<AgentPromptGetProtocol>()
        .runner(Box::new(runner(client, registry)))
}

pub fn runner(client: RuntimeClientRef, registry: PromptRegistry) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentPromptRenderProtocol>()
        .accepts::<AgentPromptGetProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |_ctx, task| {
            let registry = registry.clone();
            Box::pin(async move { run_task(registry, task).await })
        }),
    )
}

async fn run_task(registry: PromptRegistry, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_PROMPT_RENDER_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.prompt.rendered",
            |request: AgentPromptRenderRequest| registry.render(request),
        ),
        AGENT_PROMPT_GET_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.prompt.loaded",
            |request: AgentPromptGetRequest| registry.get(request),
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
