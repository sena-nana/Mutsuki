use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentContextBuildProtocol, orchestration_runner, service_result_event, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

use crate::ContextBuilder;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.context";
pub const RUNNER_ID: &str = "mutsuki.agent.context.runner";

pub fn plugin(client: RuntimeClientRef, builder: ContextBuilder) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentContextBuildProtocol>()
        .runner(Box::new(runner(client, builder)))
}

pub fn runner(client: RuntimeClientRef, builder: ContextBuilder) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentContextBuildProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |_ctx, task| {
            let builder = builder.clone();
            Box::pin(async move { run_task(builder, task).await })
        }),
    )
}

async fn run_task(builder: ContextBuilder, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_CONTEXT_BUILD_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.context.built",
            |request: AgentContextBuildRequest| builder.build(request),
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
