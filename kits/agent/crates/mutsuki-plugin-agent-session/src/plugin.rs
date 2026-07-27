use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentSessionAppendProtocol, AgentSessionCreateProtocol, AgentSessionGetProtocol,
    AgentSessionSnapshotProtocol, orchestration_runner, service_result_event, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

use crate::SessionStore;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.session";
pub const RUNNER_ID: &str = "mutsuki.agent.session.runner";

pub fn plugin(client: RuntimeClientRef, store: SessionStore) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentSessionCreateProtocol>()
        .protocol::<AgentSessionGetProtocol>()
        .protocol::<AgentSessionAppendProtocol>()
        .protocol::<AgentSessionSnapshotProtocol>()
        .runner(Box::new(runner(client, store)))
}

pub fn runner(client: RuntimeClientRef, store: SessionStore) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentSessionCreateProtocol>()
        .accepts::<AgentSessionGetProtocol>()
        .accepts::<AgentSessionAppendProtocol>()
        .accepts::<AgentSessionSnapshotProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |_ctx, task| {
            let store = store.clone();
            Box::pin(async move { run_task(store, task).await })
        }),
    )
}

async fn run_task(store: SessionStore, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_SESSION_CREATE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.session.created",
            |request: AgentSessionCreateRequest| store.create(request),
        ),
        AGENT_SESSION_GET_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.session.loaded",
            |request: AgentSessionGetRequest| store.get(request),
        ),
        AGENT_SESSION_APPEND_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.session.appended",
            |request: AgentSessionAppendRequest| store.append(request),
        ),
        AGENT_SESSION_SNAPSHOT_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.session.snapshot",
            |request: AgentSessionSnapshotRequest| store.snapshot(request),
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
