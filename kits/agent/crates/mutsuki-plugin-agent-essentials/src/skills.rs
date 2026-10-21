use mutsuki_agent_contracts::*;
use mutsuki_agent_runtime::SkillRegistry;
use mutsuki_agent_sdk::{
    AgentSkillDiscoverProtocol, AgentSkillLoadProtocol, AgentSkillReloadProtocol,
    orchestration_runner, service_result_event, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.skills";
pub const RUNNER_ID: &str = "mutsuki.agent.skills.runner";

pub fn plugin(client: RuntimeClientRef, registry: SkillRegistry) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentSkillDiscoverProtocol>()
        .protocol::<AgentSkillLoadProtocol>()
        .protocol::<AgentSkillReloadProtocol>()
        .runner(Box::new(runner(client, registry)))
}

pub fn runner(client: RuntimeClientRef, registry: SkillRegistry) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentSkillDiscoverProtocol>()
        .accepts::<AgentSkillLoadProtocol>()
        .accepts::<AgentSkillReloadProtocol>()
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

async fn run_task(registry: SkillRegistry, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_SKILL_DISCOVER_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.skill.discovered",
            |request: SkillDiscoverRequest| registry.discover(request),
        ),
        AGENT_SKILL_LOAD_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.skill.loaded",
            |request: SkillLoadRequest| registry.load(request),
        ),
        AGENT_SKILL_RELOAD_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.skill.reloaded",
            |request: SkillReloadRequest| registry.reload(request),
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
